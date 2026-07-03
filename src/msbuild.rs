use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{ProjectConfig, ToolConfig};

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
