use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{DeployTargetConfig, ToolConfig};

pub fn recycle_app_pool(tools: &ToolConfig, target: &DeployTargetConfig) -> Result<Option<String>> {
    if !target.recycle_app_pool_after_deploy {
        return Ok(None);
    }

    let app_pool = match &target.app_pool_name {
        Some(name) if !name.trim().is_empty() => name,
        _ => bail!("recycle_app_pool_after_deploy=true but app_pool_name is empty"),
    };

    let output = Command::new(&tools.appcmd_path)
        .arg("recycle")
        .arg("apppool")
        .arg(format!("/apppool.name:{}", app_pool))
        .output()
        .with_context(|| format!("Failed to start appcmd recycle for app pool '{}'", app_pool))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        bail!(
            "appcmd recycle failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            stdout,
            stderr
        );
    }

    Ok(Some(format!("{}\n{}", stdout, stderr)))
}
