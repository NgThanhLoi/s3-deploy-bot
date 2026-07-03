use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

const STORE_VERSION: u32 = 1;
const MAX_PRESETS_PER_OWNER: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FastPresetAction {
    Build,
    Deploy,
}

impl FastPresetAction {
    pub fn label(&self) -> &'static str {
        match self {
            FastPresetAction::Build => "build",
            FastPresetAction::Deploy => "deploy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastPreset {
    pub id: String,
    pub owner_user_id: i64,
    pub name: String,
    pub project: String,
    pub environment: String,
    pub branch: String,
    pub action: FastPresetAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFastPreset {
    pub name: String,
    pub project: String,
    pub environment: String,
    pub branch: String,
    pub action: FastPresetAction,
}

#[derive(Clone)]
pub struct FastPresetStore {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    presets: Vec<FastPreset>,
}

impl Default for StoreFile {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            presets: Vec::new(),
        }
    }
}

impl FastPresetStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn list_for_owner(&self, owner_user_id: i64) -> Result<Vec<FastPreset>> {
        let _guard = self.lock.lock().await;
        let store = self.read_store()?;
        Ok(store
            .presets
            .into_iter()
            .filter(|preset| preset.owner_user_id == owner_user_id)
            .collect())
    }

    pub async fn get_for_owner(
        &self,
        owner_user_id: i64,
        preset_id: &str,
    ) -> Result<Option<FastPreset>> {
        let _guard = self.lock.lock().await;
        let store = self.read_store()?;
        Ok(store
            .presets
            .into_iter()
            .find(|preset| preset.owner_user_id == owner_user_id && preset.id == preset_id))
    }

    pub async fn create(&self, owner_user_id: i64, input: NewFastPreset) -> Result<FastPreset> {
        validate_input(&input)?;

        let _guard = self.lock.lock().await;
        let mut store = self.read_store()?;
        let count = store
            .presets
            .iter()
            .filter(|preset| preset.owner_user_id == owner_user_id)
            .count();
        if count >= MAX_PRESETS_PER_OWNER {
            bail!("A user can have at most 10 fast deploy presets.");
        }

        let preset = FastPreset {
            id: Uuid::new_v4().to_string(),
            owner_user_id,
            name: input.name,
            project: input.project,
            environment: input.environment,
            branch: input.branch,
            action: input.action,
        };
        store.presets.push(preset.clone());
        self.write_store(&store)?;
        Ok(preset)
    }

    pub async fn update(
        &self,
        owner_user_id: i64,
        preset_id: &str,
        input: NewFastPreset,
    ) -> Result<FastPreset> {
        validate_input(&input)?;

        let _guard = self.lock.lock().await;
        let mut store = self.read_store()?;
        let preset = store
            .presets
            .iter_mut()
            .find(|preset| preset.owner_user_id == owner_user_id && preset.id == preset_id)
            .ok_or_else(|| anyhow!("Fast deploy preset '{}' not found.", preset_id))?;

        preset.name = input.name;
        preset.project = input.project;
        preset.environment = input.environment;
        preset.branch = input.branch;
        preset.action = input.action;
        let updated = preset.clone();
        self.write_store(&store)?;
        Ok(updated)
    }

    pub async fn delete(&self, owner_user_id: i64, preset_id: &str) -> Result<bool> {
        let _guard = self.lock.lock().await;
        let mut store = self.read_store()?;
        let before = store.presets.len();
        store
            .presets
            .retain(|preset| !(preset.owner_user_id == owner_user_id && preset.id == preset_id));
        let deleted = store.presets.len() != before;
        if deleted {
            self.write_store(&store)?;
        }
        Ok(deleted)
    }

    fn read_store(&self) -> Result<StoreFile> {
        if !self.path.exists() {
            return Ok(StoreFile::default());
        }

        let content = std::fs::read_to_string(&self.path).with_context(|| {
            format!(
                "Failed to read fast deploy preset store '{}'",
                self.path.display()
            )
        })?;
        if content.trim().is_empty() {
            return Ok(StoreFile::default());
        }

        let store: StoreFile = serde_json::from_str(&content).with_context(|| {
            format!(
                "Failed to parse fast deploy preset store '{}'",
                self.path.display()
            )
        })?;
        if store.version != STORE_VERSION {
            bail!(
                "Unsupported fast deploy preset store version {}.",
                store.version
            );
        }
        Ok(store)
    }

    fn write_store(&self, store: &StoreFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create fast deploy preset store directory '{}'",
                    parent.display()
                )
            })?;
        }

        let content = serde_json::to_string_pretty(store)
            .context("Failed to serialize fast deploy preset store")?;
        std::fs::write(&self.path, content).with_context(|| {
            format!(
                "Failed to write fast deploy preset store '{}'",
                self.path.display()
            )
        })?;
        Ok(())
    }
}

fn validate_input(input: &NewFastPreset) -> Result<()> {
    ensure_non_empty("name", &input.name)?;
    ensure_non_empty("project", &input.project)?;
    ensure_non_empty("environment", &input.environment)?;
    ensure_non_empty("branch", &input.branch)?;
    Ok(())
}

fn ensure_non_empty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("Fast deploy preset {} cannot be empty.", field);
    }
    Ok(())
}

pub fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("fast_deploy_presets.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_preset(name: &str, owner_suffix: &str) -> NewFastPreset {
        NewFastPreset {
            name: name.to_string(),
            project: format!("webpos{}", owner_suffix),
            environment: "staging".to_string(),
            branch: "s3-retail-prod".to_string(),
            action: FastPresetAction::Deploy,
        }
    }

    #[tokio::test]
    async fn fast_preset_store_lists_only_presets_for_owner() {
        let dir = tempfile::tempdir().unwrap();
        let store = FastPresetStore::new(dir.path().join("fast_deploy_presets.json"));

        store
            .create(1, new_preset("WebPOS staging", ""))
            .await
            .unwrap();
        store
            .create(2, new_preset("Other", "-other"))
            .await
            .unwrap();

        let mine = store.list_for_owner(1).await.unwrap();

        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].name, "WebPOS staging");
        assert_eq!(mine[0].owner_user_id, 1);
    }

    #[tokio::test]
    async fn fast_preset_store_updates_and_deletes_by_owner() {
        let dir = tempfile::tempdir().unwrap();
        let store = FastPresetStore::new(dir.path().join("fast_deploy_presets.json"));
        let created = store
            .create(1, new_preset("WebPOS staging", ""))
            .await
            .unwrap();

        let updated = store
            .update(
                1,
                &created.id,
                NewFastPreset {
                    name: "WebPOS build".to_string(),
                    project: "webpos".to_string(),
                    environment: "staging".to_string(),
                    branch: "develop".to_string(),
                    action: FastPresetAction::Build,
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.name, "WebPOS build");
        assert_eq!(updated.action, FastPresetAction::Build);
        assert!(store.delete(1, &created.id).await.unwrap());
        assert!(store.list_for_owner(1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fast_preset_store_rejects_more_than_ten_presets_per_owner() {
        let dir = tempfile::tempdir().unwrap();
        let store = FastPresetStore::new(dir.path().join("fast_deploy_presets.json"));

        for index in 0..10 {
            store
                .create(1, new_preset(&format!("Preset {}", index), ""))
                .await
                .unwrap();
        }

        let err = store
            .create(1, new_preset("Preset 11", ""))
            .await
            .unwrap_err();

        assert!(err.to_string().contains("at most 10"));
    }
}
