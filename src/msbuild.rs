use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{ProjectConfig, ToolConfig};

pub fn restore(tools: &ToolConfig, project: &ProjectConfig, repo_dir: &Path) -> Result<String> {
    let project_file = repo_dir.join(&project.project_file);
    let output = Command::new(&tools.nuget_path)
        .arg("restore")
        .arg(&project_file)
        .arg("-NonInteractive")
        .output()
        .with_context(|| {
            format!(
                "Failed to start NuGet restore for '{}'",
                project_file.display()
            )
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        bail!(
            "NuGet restore failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            stdout,
            stderr
        );
    }

    Ok(format!("{}\n{}", stdout, stderr))
}

pub fn publish(
    tools: &ToolConfig,
    project: &ProjectConfig,
    repo_dir: &Path,
    build_dir: &Path,
) -> Result<String> {
    let project_file = repo_dir.join(&project.project_file);
    let output = Command::new(&tools.msbuild_path)
        .arg(&project_file)
        .arg(format!("/p:Configuration={}", project.configuration))
        .arg("/p:DeployOnBuild=true")
        .arg("/p:WebPublishMethod=FileSystem")
        .arg(format!(
            "/p:PrecompileBeforePublish={}",
            bool_prop(project.precompile_before_publish)
        ))
        .arg(format!(
            "/p:EnableUpdateable={}",
            bool_prop(project.enable_updateable)
        ))
        .arg(format!("/p:PublishUrl={}", build_dir.display()))
        .output()
        .with_context(|| format!("Failed to start MSBuild for '{}'", project_file.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        bail!(
            "MSBuild failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            stdout,
            stderr
        );
    }

    Ok(format!("{}\n{}", stdout, stderr))
}

fn bool_prop(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    #[test]
    #[cfg(unix)]
    fn restore_runs_nuget_restore_for_project_file() {
        let dir = tempfile::tempdir().unwrap();
        let nuget_log = dir.path().join("nuget.log");
        let nuget = dir.path().join("nuget_fake.sh");
        std::fs::write(
            &nuget,
            format!(
                "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > '{}'\n",
                nuget_log.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&nuget).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&nuget, permissions).unwrap();

        let repo_dir = dir.path().join("repo");
        let project_dir = repo_dir.join("Websites/WebPOS");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(project_dir.join("WebPOS.csproj"), "<Project />").unwrap();
        let tools = ToolConfig {
            git_path: PathBuf::from("git"),
            msbuild_path: PathBuf::from("msbuild"),
            nuget_path: nuget,
            robocopy_path: PathBuf::from("robocopy"),
            seven_zip_path: PathBuf::from("7z"),
            appcmd_path: PathBuf::from("appcmd"),
        };
        let project = ProjectConfig {
            key: "webpos".to_string(),
            name: "WebPOS".to_string(),
            repository: "s3retail".to_string(),
            project_file: PathBuf::from("Websites/WebPOS/WebPOS.csproj"),
            configuration: "Release".to_string(),
            precompile_before_publish: true,
            enable_updateable: true,
            delete_from_build: vec![],
        };

        restore(&tools, &project, &repo_dir).unwrap();

        let log = std::fs::read_to_string(nuget_log).unwrap();
        assert!(log.contains("restore"));
        assert!(log.contains("WebPOS.csproj"));
        assert!(log.contains("-NonInteractive"));
    }
}
