use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{RepositoryConfig, ToolConfig};

pub fn clone_branch(
    tools: &ToolConfig,
    repo: &RepositoryConfig,
    branch: &str,
    repo_dir: &Path,
) -> Result<String> {
    let output = Command::new(&tools.git_path)
        .arg("clone")
        .arg("--branch")
        .arg(branch)
        .arg("--single-branch")
        .arg(&repo.repo_url)
        .arg(repo_dir)
        .output()
        .with_context(|| format!("Failed to start git clone for repository '{}'", repo.key))?;

    if !output.status.success() {
        bail!(
            "git clone failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    rev_parse(tools, repo_dir)
}

pub fn rev_parse(tools: &ToolConfig, repo_dir: &Path) -> Result<String> {
    let output = Command::new(&tools.git_path)
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(repo_dir)
        .output()
        .with_context(|| format!("Failed to start git rev-parse in '{}'", repo_dir.display()))?;

    if !output.status.success() {
        bail!(
            "git rev-parse failed with status {:?}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
