use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{DeployTargetConfig, ToolConfig};

pub fn copy_overlay(
    tools: &ToolConfig,
    build_dir: &Path,
    target: &DeployTargetConfig,
) -> Result<String> {
    let output = Command::new(&tools.robocopy_path)
        .arg(build_dir)
        .arg(&target.iis_path)
        .arg("/E")
        .output()
        .with_context(|| {
            format!(
                "Failed to start robocopy from '{}' to '{}'",
                build_dir.display(),
                target.iis_path.display()
            )
        })?;

    let code = output.status.code().unwrap_or(16);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if code > 7 {
        bail!(
            "robocopy failed with exit code {}\nstdout:\n{}\nstderr:\n{}",
            code,
            stdout,
            stderr
        );
    }

    Ok(format!(
        "robocopy exit code {}\nstdout:\n{}\nstderr:\n{}",
        code, stdout, stderr
    ))
}
