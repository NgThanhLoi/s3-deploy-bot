use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{RepositoryConfig, ToolConfig};

fn git_command(tools: &ToolConfig) -> Command {
    let mut command = Command::new(&tools.git_path);
    command.arg("-c").arg("core.longpaths=true");
    command
}

fn mirror_branch_ref(branch: &str) -> String {
    format!("refs/heads/{}", branch)
}

pub fn checkout_fresh_worktree(
    tools: &ToolConfig,
    repo: &RepositoryConfig,
    branch: &str,
    mirror_dir: &Path,
    worktree_dir: &Path,
) -> Result<String> {
    ensure_mirror(tools, repo, mirror_dir)?;
    fetch_mirror(tools, mirror_dir)?;
    prune_stale_worktrees(tools, mirror_dir)?;
    let commit = resolve_remote_branch(tools, mirror_dir, branch)?;
    add_worktree(tools, mirror_dir, worktree_dir, &commit)?;
    Ok(commit)
}

pub fn remove_worktree(tools: &ToolConfig, mirror_dir: &Path, worktree_dir: &Path) -> Result<()> {
    if !worktree_dir.exists() {
        return Ok(());
    }

    let output = git_command(tools)
        .arg("--git-dir")
        .arg(mirror_dir)
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(worktree_dir)
        .output()
        .with_context(|| {
            format!(
                "Failed to start git worktree remove for '{}'",
                worktree_dir.display()
            )
        })?;

    if !output.status.success() {
        bail!(
            "git worktree remove failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

fn ensure_mirror(tools: &ToolConfig, repo: &RepositoryConfig, mirror_dir: &Path) -> Result<()> {
    if mirror_dir.join("HEAD").is_file() {
        return Ok(());
    }

    if let Some(parent) = mirror_dir.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create repository cache dir '{}'",
                parent.display()
            )
        })?;
    }

    let output = git_command(tools)
        .arg("clone")
        .arg("--mirror")
        .arg(&repo.repo_url)
        .arg(mirror_dir)
        .output()
        .with_context(|| format!("Failed to start git clone --mirror for '{}'", repo.key))?;

    if !output.status.success() {
        bail!(
            "git clone --mirror failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

fn fetch_mirror(tools: &ToolConfig, mirror_dir: &Path) -> Result<()> {
    let output = git_command(tools)
        .arg("--git-dir")
        .arg(mirror_dir)
        .arg("fetch")
        .arg("--prune")
        .arg("origin")
        .output()
        .with_context(|| format!("Failed to start git fetch in '{}'", mirror_dir.display()))?;

    if !output.status.success() {
        bail!(
            "git fetch failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

fn prune_stale_worktrees(tools: &ToolConfig, mirror_dir: &Path) -> Result<()> {
    let output = git_command(tools)
        .arg("--git-dir")
        .arg(mirror_dir)
        .arg("worktree")
        .arg("prune")
        .output()
        .with_context(|| {
            format!(
                "Failed to start git worktree prune in '{}'",
                mirror_dir.display()
            )
        })?;

    if !output.status.success() {
        bail!(
            "git worktree prune failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

fn resolve_remote_branch(tools: &ToolConfig, mirror_dir: &Path, branch: &str) -> Result<String> {
    let branch_ref = mirror_branch_ref(branch);
    let output = git_command(tools)
        .arg("--git-dir")
        .arg(mirror_dir)
        .arg("rev-parse")
        .arg(&branch_ref)
        .output()
        .with_context(|| {
            format!(
                "Failed to start git rev-parse for '{}' in '{}'",
                branch_ref,
                mirror_dir.display()
            )
        })?;

    if !output.status.success() {
        bail!(
            "git rev-parse '{}' failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            branch_ref,
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn add_worktree(
    tools: &ToolConfig,
    mirror_dir: &Path,
    worktree_dir: &Path,
    commit: &str,
) -> Result<()> {
    if worktree_dir.exists() {
        std::fs::remove_dir_all(worktree_dir).with_context(|| {
            format!(
                "Failed to remove existing worktree dir '{}'",
                worktree_dir.display()
            )
        })?;
    }

    let output = git_command(tools)
        .arg("--git-dir")
        .arg(mirror_dir)
        .arg("worktree")
        .arg("add")
        .arg("--detach")
        .arg("--force")
        .arg(worktree_dir)
        .arg(commit)
        .output()
        .with_context(|| {
            format!(
                "Failed to start git worktree add for '{}'",
                worktree_dir.display()
            )
        })?;

    if !output.status.success() {
        bail!(
            "git worktree add failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tools() -> ToolConfig {
        ToolConfig {
            git_path: PathBuf::from("git"),
            msbuild_path: PathBuf::from("msbuild"),
            robocopy_path: PathBuf::from("robocopy"),
            appcmd_path: PathBuf::from("appcmd"),
            seven_zip_path: PathBuf::from("7z"),
        }
    }

    #[test]
    fn git_command_enables_windows_long_paths() {
        let command = git_command(&tools());
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(args, vec!["-c", "core.longpaths=true"]);
    }

    #[test]
    fn mirror_branch_ref_uses_heads_namespace() {
        assert_eq!(
            mirror_branch_ref("s3-retail-prod"),
            "refs/heads/s3-retail-prod"
        );
    }
}
