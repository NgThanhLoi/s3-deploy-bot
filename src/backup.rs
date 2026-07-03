use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

use crate::config::{DeployTargetConfig, ProjectConfig};

pub fn backup_iis(
    project: &ProjectConfig,
    target: &DeployTargetConfig,
    environment_key: &str,
) -> Result<PathBuf> {
    let now = Local::now();
    let dir = target
        .backup_root
        .join(environment_key)
        .join(now.format("%Y-%m-%d").to_string());
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create backup dir '{}'", dir.display()))?;

    let zip_path = dir.join(format!("{}-{}.zip", project.key, now.format("%H-%M-%S")));
    zip_dir(&target.iis_path, &zip_path)?;
    Ok(zip_path)
}

fn zip_dir(source_dir: &Path, zip_path: &Path) -> Result<()> {
    let file = File::create(zip_path)
        .with_context(|| format!("Failed to create backup zip '{}'", zip_path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut buffer = Vec::new();

    for entry in WalkDir::new(source_dir) {
        let entry = entry.with_context(|| {
            format!(
                "Failed to walk source directory while backing up '{}'",
                source_dir.display()
            )
        })?;
        let path = entry.path();
        let name = path.strip_prefix(source_dir).with_context(|| {
            format!(
                "Failed to strip backup source prefix '{}' from '{}'",
                source_dir.display(),
                path.display()
            )
        })?;

        if name.as_os_str().is_empty() {
            continue;
        }

        let name = name.to_string_lossy().replace('\\', "/");
        if path.is_file() {
            zip.start_file(name, options)?;
            let mut f = File::open(path)
                .with_context(|| format!("Failed to open file '{}'", path.display()))?;
            f.read_to_end(&mut buffer)?;
            zip.write_all(&buffer)?;
            buffer.clear();
        } else if path.is_dir() {
            zip.add_directory(format!("{}/", name.trim_end_matches('/')), options)?;
        }
    }

    zip.finish()?;
    Ok(())
}
