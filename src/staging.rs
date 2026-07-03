use std::path::{Component, Path};

use anyhow::{bail, Context, Result};

pub fn clean_build_dir(build_dir: &Path) -> Result<()> {
    if build_dir.exists() {
        std::fs::remove_dir_all(build_dir)
            .with_context(|| format!("Failed to remove build dir '{}'", build_dir.display()))?;
    }
    std::fs::create_dir_all(build_dir)
        .with_context(|| format!("Failed to create build dir '{}'", build_dir.display()))?;
    Ok(())
}

pub fn delete_entries(build_dir: &Path, entries: &[String]) -> Result<Vec<String>> {
    let mut deleted = Vec::new();
    for entry in entries {
        validate_relative_entry(entry)?;
        let path = build_dir.join(entry);
        if path.is_dir() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("Failed to remove directory '{}'", path.display()))?;
            deleted.push(entry.clone());
        } else if path.is_file() {
            std::fs::remove_file(&path)
                .with_context(|| format!("Failed to remove file '{}'", path.display()))?;
            deleted.push(entry.clone());
        }
    }
    Ok(deleted)
}

fn validate_relative_entry(entry: &str) -> Result<()> {
    let path = Path::new(entry);
    if entry.trim().is_empty() {
        bail!("Build cleanup entry cannot be empty.");
    }
    if path.is_absolute() {
        bail!("Build cleanup entry '{}' must be relative.", entry);
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("Build cleanup entry '{}' must not contain '..'.", entry);
    }
    Ok(())
}
