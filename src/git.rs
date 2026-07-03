use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{RepositoryConfig, ToolConfig};

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

    let output = Command::new(&tools.git_path)
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

    let output = Command::new(&tools.git_path)
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
    let output = Command::new(&tools.git_path)
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
    let output = Command::new(&tools.git_path)
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
    let output = Command::new(&tools.git_path)
        .arg("--git-dir")
        .arg(mirror_dir)
        .arg("rev-parse")
        .arg(format!("refs/remotes/origin/{}", branch))
        .output()
        .with_context(|| {
            format!(
                "Failed to start git rev-parse for origin/{} in '{}'",
                branch,
                mirror_dir.display()
            )
        })?;

    if !output.status.success() {
        bail!(
            "git rev-parse origin/{} failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            branch,
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

    let output = Command::new(&tools.git_path)
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
