use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Raw config structs (directly deserialized from TOML)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub name: String,
    pub timezone: String,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    pub bot_token_env: String,
    pub allowed_chat_ids: Vec<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserConfig {
    pub id: i64,
    pub name: String,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RolePermissions {
    pub can_build: bool,
    pub can_deploy_staging: bool,
    pub can_deploy_production: bool,
    pub can_rollback: bool,
    pub can_view_logs: bool,
    pub can_cancel_jobs: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolConfig {
    pub git_path: PathBuf,
    pub msbuild_path: PathBuf,
    pub robocopy_path: PathBuf,
    pub seven_zip_path: PathBuf,
    pub appcmd_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DefaultsConfig {
    pub build_timeout_minutes: u64,
    pub deploy_timeout_minutes: u64,
    pub backup_timeout_minutes: u64,
    pub max_log_lines_in_telegram: usize,
    pub project_lock_timeout_minutes: u64,
    pub keep_staging_on_failure: bool,
    pub keep_success_staging: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvironmentConfig {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub requires_double_confirm: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuickDeployConfig {
    #[serde(default)]
    pub enabled: bool,
    pub project: String,
    pub environment: String,
    pub branch: String,
    #[serde(default = "default_quick_action")]
    pub action: String,
}

fn default_quick_action() -> String {
    "deploy".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepositoryConfig {
    pub key: String,
    pub name: String,
    pub repo_url: String,
    pub main_branch: String,
    #[serde(default)]
    pub quick_branches: Vec<String>,
    #[serde(default = "default_true")]
    pub manual_branch_enabled: bool,
    #[serde(default)]
    pub manual_branch_patterns: Vec<String>,
    #[serde(default)]
    pub forbidden_branch_patterns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    pub key: String,
    pub name: String,
    pub repository: String,
    pub project_file: PathBuf,
    pub configuration: String,
    #[serde(default)]
    pub precompile_before_publish: bool,
    #[serde(default = "default_true")]
    pub enable_updateable: bool,
    #[serde(default)]
    pub delete_from_build: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeployTargetConfig {
    pub project: String,
    pub environment: String,
    pub iis_path: PathBuf,
    pub backup_root: PathBuf,
    pub deploy_mode: String,
    pub use_app_offline: bool,
    pub recycle_app_pool_after_deploy: bool,
    pub app_pool_name: Option<String>,
    #[serde(default)]
    pub preserve_files: Vec<String>,
    #[serde(default)]
    pub preserve_dirs: Vec<String>,
}

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct RawConfig {
    pub app: AppConfig,
    pub telegram: TelegramConfig,
    pub users: Vec<UserConfig>,
    pub roles: HashMap<String, RolePermissions>,
    pub tools: ToolConfig,
    pub defaults: DefaultsConfig,
    pub quick_deploy: Option<QuickDeployConfig>,
    pub environments: Vec<EnvironmentConfig>,
    pub repositories: Vec<RepositoryConfig>,
    pub projects: Vec<ProjectConfig>,
    pub deploy_targets: Vec<DeployTargetConfig>,
}

// ---------------------------------------------------------------------------
// Validated config (same shape, validated)
// ---------------------------------------------------------------------------

pub type Config = RawConfig;

// ---------------------------------------------------------------------------
// Loading & validation
// ---------------------------------------------------------------------------

impl RawConfig {
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: RawConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        // 1. bot_token_env must exist in environment
        let token = env::var(&self.telegram.bot_token_env).map_err(|_| {
            anyhow::anyhow!(
                "Environment variable '{}' is not set. Please set it before running the bot.",
                self.telegram.bot_token_env
            )
        })?;
        if token.trim().is_empty() {
            bail!(
                "Environment variable '{}' is set but empty.",
                self.telegram.bot_token_env
            );
        }

        // 2. At least 1 user
        ensure!(
            !self.users.is_empty(),
            "Config must have at least 1 [[users]] entry."
        );

        // 3. At least 1 admin
        let admin_count = self.users.iter().filter(|u| u.role == "admin").count();
        ensure!(
            admin_count > 0,
            "Config must have at least 1 user with role = \"admin\"."
        );

        // 4. Role of each user must exist in [roles]
        for user in &self.users {
            ensure!(
                self.roles.contains_key(&user.role),
                "User '{}' has unknown role '{}'. Role must be defined in [roles] section.",
                user.name,
                user.role
            );
        }

        // 5. Repository keys must be unique
        let mut repository_keys_seen = HashSet::new();
        for repo in &self.repositories {
            if !repository_keys_seen.insert(&repo.key) {
                bail!(
                    "Duplicate repository key '{}'. Each [[repositories]] must have a unique key.",
                    repo.key
                );
            }
        }

        // 6. Project keys must be unique
        let mut project_keys_seen = HashSet::new();
        for project in &self.projects {
            if !project_keys_seen.insert(&project.key) {
                bail!(
                    "Duplicate project key '{}'. Each [[projects]] must have a unique key.",
                    project.key
                );
            }
        }

        // 7. Environment keys must be unique
        let mut env_keys_seen = HashSet::new();
        for env in &self.environments {
            if !env_keys_seen.insert(&env.key) {
                bail!(
                    "Duplicate environment key '{}'. Each [[environments]] must have a unique key.",
                    env.key
                );
            }
        }

        // Build lookup maps for validation
        let repository_keys: HashSet<&str> =
            self.repositories.iter().map(|r| r.key.as_str()).collect();
        let project_keys: HashSet<&str> = self.projects.iter().map(|p| p.key.as_str()).collect();
        let env_keys: HashSet<&str> = self.environments.iter().map(|e| e.key.as_str()).collect();

        // 8. Projects must reference valid repositories
        for project in &self.projects {
            ensure!(
                repository_keys.contains(project.repository.as_str()),
                "Project '{}' references unknown repository key '{}'. Valid keys: {:?}",
                project.key,
                project.repository,
                repository_keys
            );
            ensure!(
                !project.project_file.is_absolute(),
                "Project '{}' has project_file='{}'. project_file must be relative to the cloned repository root.",
                project.key,
                project.project_file.display()
            );
            validate_relative_entries(
                &project.delete_from_build,
                &format!("project '{}'.delete_from_build", project.key),
            )?;
        }

        // 9. Deploy targets must reference valid project and environment
        for dt in &self.deploy_targets {
            ensure!(
                project_keys.contains(dt.project.as_str()),
                "Deploy target references unknown project key '{}'. Valid keys: {:?}",
                dt.project,
                project_keys
            );
            ensure!(
                env_keys.contains(dt.environment.as_str()),
                "Deploy target references unknown environment key '{}'. Valid keys: {:?}",
                dt.environment,
                env_keys
            );
        }

        // 10. Quick deploy must reference a valid target when enabled
        if let Some(quick) = &self.quick_deploy {
            if quick.enabled {
                ensure!(
                    matches!(quick.action.as_str(), "build" | "deploy"),
                    "quick_deploy.action must be 'build' or 'deploy', got '{}'.",
                    quick.action
                );
                ensure!(
                    project_keys.contains(quick.project.as_str()),
                    "quick_deploy.project references unknown project key '{}'.",
                    quick.project
                );
                ensure!(
                    env_keys.contains(quick.environment.as_str()),
                    "quick_deploy.environment references unknown environment key '{}'.",
                    quick.environment
                );
                ensure!(
                    self.deploy_targets.iter().any(
                        |dt| dt.project == quick.project && dt.environment == quick.environment
                    ),
                    "quick_deploy target project='{}' environment='{}' has no deploy target.",
                    quick.project,
                    quick.environment
                );

                let project = self
                    .projects
                    .iter()
                    .find(|p| p.key == quick.project)
                    .expect("project key was validated");
                let repo = self
                    .repositories
                    .iter()
                    .find(|r| r.key == project.repository)
                    .expect("project repository was validated");
                ensure!(
                    quick.branch == repo.main_branch
                        || repo.quick_branches.iter().any(|b| b == &quick.branch),
                    "quick_deploy.branch '{}' must be repository '{}' main_branch or one of quick_branches.",
                    quick.branch,
                    repo.key
                );
            }
        }

        // 11. main_branch must not be empty
        for repo in &self.repositories {
            ensure!(
                !repo.main_branch.is_empty(),
                "Repository '{}' has an empty main_branch.",
                repo.key
            );
        }

        // 12. deploy_mode only accepts "overlay" in MVP
        for dt in &self.deploy_targets {
            ensure!(
                dt.deploy_mode == "overlay",
                "Deploy target (project='{}', env='{}') has deploy_mode='{}'. Only 'overlay' is supported in MVP.",
                dt.project, dt.environment, dt.deploy_mode
            );
        }

        // 13. use_app_offline must be false in MVP
        for dt in &self.deploy_targets {
            ensure!(
                !dt.use_app_offline,
                "Deploy target (project='{}', env='{}') has use_app_offline=true. Not supported in MVP.",
                dt.project, dt.environment
            );
        }

        // 14. Ensure required directories can be created
        ensure_dir_creatable(&self.app.data_dir, "app.data_dir")?;
        ensure_dir_creatable(&self.app.log_dir, "app.log_dir")?;
        ensure_dir_creatable(&self.app.workspace_root, "app.workspace_root")?;
        for project in &self.projects {
            let matching_targets = self
                .deploy_targets
                .iter()
                .filter(|dt| dt.project == project.key);
            for dt in matching_targets {
                ensure_dir_creatable(
                    &dt.backup_root,
                    &format!(
                        "deploy_target (project='{}', env='{}').backup_root",
                        dt.project, dt.environment
                    ),
                )?;
            }
        }

        Ok(())
    }
}

fn ensure_dir_creatable(path: &Path, label: &str) -> Result<()> {
    if path.exists() {
        ensure!(
            path.is_dir(),
            "{} ('{}') exists but is not a directory.",
            label,
            path.display()
        );
    } else {
        std::fs::create_dir_all(path).with_context(|| {
            format!("Cannot create directory {} ('{}').", label, path.display())
        })?;
    }
    Ok(())
}

fn validate_relative_entries(entries: &[String], label: &str) -> Result<()> {
    for entry in entries {
        let path = Path::new(entry);
        ensure!(
            !entry.trim().is_empty(),
            "{} contains an empty path entry.",
            label
        );
        ensure!(
            !path.is_absolute(),
            "{} contains absolute path '{}'. Entries must be relative to the build root.",
            label,
            entry
        );
        ensure!(
            !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "{} contains path traversal '{}'.",
            label,
            entry
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn get_bot_token(config: &Config) -> Result<String> {
    env::var(&config.telegram.bot_token_env).with_context(|| {
        format!(
            "Environment variable '{}' is not set.",
            config.telegram.bot_token_env
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct TempConfig {
        _file: tempfile::NamedTempFile,
        path: std::path::PathBuf,
    }

    fn write_temp_config(content: &str) -> TempConfig {
        let mut file = tempfile::NamedTempFile::new().expect("create temp file");
        write!(file, "{}", content).expect("write temp config");
        let path = file.path().to_path_buf();
        TempConfig { _file: file, path }
    }

    #[test]
    fn minimal_valid_config() {
        let toml = r#"
[app]
name = "Test Bot"
timezone = "Asia/Ho_Chi_Minh"
data_dir = "/tmp/testbot/data"
log_dir = "/tmp/testbot/logs"
workspace_root = "/tmp/test/workspace-root"

[telegram]
bot_token_env = "TEST_BOT_TOKEN"
allowed_chat_ids = [123]

[[users]]
id = 123
name = "Admin"
role = "admin"

[[users]]
id = 456
name = "Dev"
role = "developer"

[roles.admin]
can_build = true
can_deploy_staging = true
can_deploy_production = true
can_rollback = true
can_view_logs = true
can_cancel_jobs = true

[roles.developer]
can_build = true
can_deploy_staging = true
can_deploy_production = false
can_rollback = false
can_view_logs = true
can_cancel_jobs = false

[tools]
git_path = "git"
msbuild_path = "msbuild"
robocopy_path = "robocopy"
seven_zip_path = "7z"
appcmd_path = "appcmd"

[defaults]
build_timeout_minutes = 30
deploy_timeout_minutes = 15
backup_timeout_minutes = 30
max_log_lines_in_telegram = 80
project_lock_timeout_minutes = 60
keep_staging_on_failure = true
keep_success_staging = false

[[environments]]
key = "staging"
name = "Staging"
requires_double_confirm = false

[[environments]]
key = "production"
name = "Production"

[[repositories]]
key = "repo"
name = "Repo"
repo_url = "git@github.com:test/repo.git"
main_branch = "master"
quick_branches = ["master", "develop"]
manual_branch_enabled = true
manual_branch_patterns = ["feature/*", "bugfix/*", "hotfix/*", "release/*", "dev/*"]
forbidden_branch_patterns = ["backup/*"]

[[projects]]
key = "webpos"
name = "WebPOS"
repository = "repo"
repo_url = "git@github.com:test/webpos.git"
workspace = "/tmp/testbot/workspace/webpos"
project_file = "WebPOS.csproj"
configuration = "Release"
main_branch = "master"
quick_branches = ["master", "develop"]
manual_branch_enabled = true
manual_branch_patterns = ["feature/*", "bugfix/*"]
forbidden_branch_patterns = ["backup/*"]

[[deploy_targets]]
project = "webpos"
environment = "production"
iis_path = "/tmp/testbot/iis/webpos"
publish_root = "/tmp/testbot/publish/webpos"
backup_root = "/tmp/testbot/backup/webpos"
deploy_mode = "overlay"
use_app_offline = false
recycle_app_pool_after_deploy = true
app_pool_name = "WebPOS"
"#;
        std::env::set_var("TEST_BOT_TOKEN", "test-token-123");
        let t = write_temp_config(toml);
        let config = RawConfig::from_file(&t.path);
        assert!(
            config.is_ok(),
            "Config should be valid, got: {:?}",
            config.err()
        );
        std::env::remove_var("TEST_BOT_TOKEN");
    }

    #[test]
    fn missing_bot_token_env() {
        let toml = r#"
[app]
name = "Test"
timezone = "UTC"
data_dir = "/tmp/test/data"
log_dir = "/tmp/test/logs"
workspace_root = "/tmp/test/workspace-root"

[telegram]
bot_token_env = "DOES_NOT_EXIST"
allowed_chat_ids = [123]

[[users]]
id = 123
name = "Admin"
role = "admin"

[roles.admin]
can_build = true
can_deploy_staging = true
can_deploy_production = true
can_rollback = true
can_view_logs = true
can_cancel_jobs = true

[tools]
git_path = "git"
msbuild_path = "msbuild"
robocopy_path = "robocopy"
seven_zip_path = "7z"
appcmd_path = "appcmd"

[defaults]
build_timeout_minutes = 30
deploy_timeout_minutes = 15
backup_timeout_minutes = 30
max_log_lines_in_telegram = 80
project_lock_timeout_minutes = 60
keep_staging_on_failure = true
keep_success_staging = false

[[environments]]
key = "staging"
name = "Staging"

[[repositories]]
key = "repo"
name = "Repo"
repo_url = "git@github.com:test/repo.git"
main_branch = ""
quick_branches = ["master", "develop"]
manual_branch_enabled = true
manual_branch_patterns = ["feature/*", "bugfix/*", "hotfix/*", "release/*", "dev/*"]
forbidden_branch_patterns = ["backup/*"]

[[projects]]
key = "webpos"
name = "WebPOS"
repository = "repo"
repo_url = "git@github.com:test/webpos.git"
workspace = "/tmp/test/workspace"
project_file = "WebPOS.csproj"
configuration = "Release"
main_branch = "master"

[[deploy_targets]]
project = "webpos"
environment = "staging"
iis_path = "/tmp/test/iis"
publish_root = "/tmp/test/publish"
backup_root = "/tmp/test/backup"
deploy_mode = "overlay"
use_app_offline = false
recycle_app_pool_after_deploy = false
"#;
        let t = write_temp_config(toml);
        let config = RawConfig::from_file(&t.path);
        assert!(config.is_err(), "Config should fail due to missing env var");
        let err = format!("{}", config.err().unwrap());
        assert!(
            err.contains("DOES_NOT_EXIST"),
            "Error should mention the env var name: {}",
            err
        );
    }

    #[test]
    fn duplicate_project_key_fails() {
        let toml = r#"
[app]
name = "Test"
timezone = "UTC"
data_dir = "/tmp/test/data"
log_dir = "/tmp/test/logs"
workspace_root = "/tmp/test/workspace-root"

[telegram]
bot_token_env = "TEST_BOT_TOKEN_2"
allowed_chat_ids = [123]

[[users]]
id = 123
name = "Admin"
role = "admin"

[roles.admin]
can_build = true
can_deploy_staging = true
can_deploy_production = true
can_rollback = true
can_view_logs = true
can_cancel_jobs = true

[tools]
git_path = "git"
msbuild_path = "msbuild"
robocopy_path = "robocopy"
seven_zip_path = "7z"
appcmd_path = "appcmd"

[defaults]
build_timeout_minutes = 30
deploy_timeout_minutes = 15
backup_timeout_minutes = 30
max_log_lines_in_telegram = 80
project_lock_timeout_minutes = 60
keep_staging_on_failure = true
keep_success_staging = false

[[environments]]
key = "staging"
name = "Staging"

[[repositories]]
key = "repo"
name = "Repo"
repo_url = "git@github.com:test/repo.git"
main_branch = "master"
quick_branches = ["master", "develop"]
manual_branch_enabled = true
manual_branch_patterns = ["feature/*", "bugfix/*", "hotfix/*", "release/*", "dev/*"]
forbidden_branch_patterns = ["backup/*"]

[[projects]]
key = "webpos"
name = "WebPOS"
repository = "repo"
repo_url = "git@github.com:test/a.git"
workspace = "/tmp/test/a"
project_file = "WebPOS.csproj"
configuration = "Release"
main_branch = "master"

[[projects]]
key = "webpos"
name = "WebPOS Duplicate"
repository = "repo"
repo_url = "git@github.com:test/b.git"
workspace = "/tmp/test/b"
project_file = "WebPOS.csproj"
configuration = "Release"
main_branch = "main"

[[deploy_targets]]
project = "webpos"
environment = "staging"
iis_path = "/tmp/test/iis"
publish_root = "/tmp/test/publish"
backup_root = "/tmp/test/backup"
deploy_mode = "overlay"
use_app_offline = false
recycle_app_pool_after_deploy = false
"#;
        std::env::set_var("TEST_BOT_TOKEN_2", "token-2");
        let t = write_temp_config(toml);
        let config = RawConfig::from_file(&t.path);
        assert!(config.is_err(), "Should fail on duplicate project key");
        let err = format!("{}", config.err().unwrap());
        assert!(err.contains("Duplicate project key"), "Error: {}", err);
        std::env::remove_var("TEST_BOT_TOKEN_2");
    }

    #[test]
    fn deploy_mode_must_be_overlay() {
        let toml = r#"
[app]
name = "Test"
timezone = "UTC"
data_dir = "/tmp/test/data"
log_dir = "/tmp/test/logs"
workspace_root = "/tmp/test/workspace-root"

[telegram]
bot_token_env = "TEST_BOT_TOKEN_3"
allowed_chat_ids = [123]

[[users]]
id = 123
name = "Admin"
role = "admin"

[roles.admin]
can_build = true
can_deploy_staging = true
can_deploy_production = true
can_rollback = true
can_view_logs = true
can_cancel_jobs = true

[tools]
git_path = "git"
msbuild_path = "msbuild"
robocopy_path = "robocopy"
seven_zip_path = "7z"
appcmd_path = "appcmd"

[defaults]
build_timeout_minutes = 30
deploy_timeout_minutes = 15
backup_timeout_minutes = 30
max_log_lines_in_telegram = 80
project_lock_timeout_minutes = 60
keep_staging_on_failure = true
keep_success_staging = false

[[environments]]
key = "staging"
name = "Staging"

[[repositories]]
key = "repo"
name = "Repo"
repo_url = "git@github.com:test/repo.git"
main_branch = "master"
quick_branches = ["master", "develop"]
manual_branch_enabled = true
manual_branch_patterns = ["feature/*", "bugfix/*", "hotfix/*", "release/*", "dev/*"]
forbidden_branch_patterns = ["backup/*"]

[[projects]]
key = "webpos"
name = "WebPOS"
repository = "repo"
repo_url = "git@github.com:test/webpos.git"
workspace = "/tmp/test/workspace"
project_file = "WebPOS.csproj"
configuration = "Release"
main_branch = "master"

[[deploy_targets]]
project = "webpos"
environment = "staging"
iis_path = "/tmp/test/iis"
publish_root = "/tmp/test/publish"
backup_root = "/tmp/test/backup"
deploy_mode = "mirror"
use_app_offline = false
recycle_app_pool_after_deploy = false
"#;
        std::env::set_var("TEST_BOT_TOKEN_3", "token-3");
        let t = write_temp_config(toml);
        let config = RawConfig::from_file(&t.path);
        assert!(config.is_err(), "Should fail on mirror mode");
        let err = format!("{}", config.err().unwrap());
        assert!(err.contains("deploy_mode"), "Error: {}", err);
        std::env::remove_var("TEST_BOT_TOKEN_3");
    }

    #[test]
    fn use_app_offline_must_be_false() {
        let toml = r#"
[app]
name = "Test"
timezone = "UTC"
data_dir = "/tmp/test/data"
log_dir = "/tmp/test/logs"
workspace_root = "/tmp/test/workspace-root"

[telegram]
bot_token_env = "TEST_BOT_TOKEN_4"
allowed_chat_ids = [123]

[[users]]
id = 123
name = "Admin"
role = "admin"

[roles.admin]
can_build = true
can_deploy_staging = true
can_deploy_production = true
can_rollback = true
can_view_logs = true
can_cancel_jobs = true

[tools]
git_path = "git"
msbuild_path = "msbuild"
robocopy_path = "robocopy"
seven_zip_path = "7z"
appcmd_path = "appcmd"

[defaults]
build_timeout_minutes = 30
deploy_timeout_minutes = 15
backup_timeout_minutes = 30
max_log_lines_in_telegram = 80
project_lock_timeout_minutes = 60
keep_staging_on_failure = true
keep_success_staging = false

[[environments]]
key = "staging"
name = "Staging"

[[repositories]]
key = "repo"
name = "Repo"
repo_url = "git@github.com:test/repo.git"
main_branch = "master"
quick_branches = ["master", "develop"]
manual_branch_enabled = true
manual_branch_patterns = ["feature/*", "bugfix/*", "hotfix/*", "release/*", "dev/*"]
forbidden_branch_patterns = ["backup/*"]

[[projects]]
key = "webpos"
name = "WebPOS"
repository = "repo"
repo_url = "git@github.com:test/webpos.git"
workspace = "/tmp/test/workspace"
project_file = "WebPOS.csproj"
configuration = "Release"
main_branch = "master"

[[deploy_targets]]
project = "webpos"
environment = "staging"
iis_path = "/tmp/test/iis"
publish_root = "/tmp/test/publish"
backup_root = "/tmp/test/backup"
deploy_mode = "overlay"
use_app_offline = true
recycle_app_pool_after_deploy = false
"#;
        std::env::set_var("TEST_BOT_TOKEN_4", "token-4");
        let t = write_temp_config(toml);
        let config = RawConfig::from_file(&t.path);
        assert!(config.is_err(), "Should fail on use_app_offline=true");
        let err = format!("{}", config.err().unwrap());
        assert!(err.contains("use_app_offline"), "Error: {}", err);
        std::env::remove_var("TEST_BOT_TOKEN_4");
    }

    #[test]
    fn empty_main_branch_fails() {
        let toml = r#"
[app]
name = "Test"
timezone = "UTC"
data_dir = "/tmp/test/data"
log_dir = "/tmp/test/logs"
workspace_root = "/tmp/test/workspace-root"

[telegram]
bot_token_env = "TEST_BOT_TOKEN_5"
allowed_chat_ids = [123]

[[users]]
id = 123
name = "Admin"
role = "admin"

[roles.admin]
can_build = true
can_deploy_staging = true
can_deploy_production = true
can_rollback = true
can_view_logs = true
can_cancel_jobs = true

[tools]
git_path = "git"
msbuild_path = "msbuild"
robocopy_path = "robocopy"
seven_zip_path = "7z"
appcmd_path = "appcmd"

[defaults]
build_timeout_minutes = 30
deploy_timeout_minutes = 15
backup_timeout_minutes = 30
max_log_lines_in_telegram = 80
project_lock_timeout_minutes = 60
keep_staging_on_failure = true
keep_success_staging = false

[[environments]]
key = "staging"
name = "Staging"

[[repositories]]
key = "repo"
name = "Repo"
repo_url = "git@github.com:test/repo.git"
main_branch = ""
quick_branches = ["master", "develop"]
manual_branch_enabled = true
manual_branch_patterns = ["feature/*", "bugfix/*", "hotfix/*", "release/*", "dev/*"]
forbidden_branch_patterns = ["backup/*"]

[[projects]]
key = "webpos"
name = "WebPOS"
repository = "repo"
repo_url = "git@github.com:test/webpos.git"
workspace = "/tmp/test/workspace"
project_file = "WebPOS.csproj"
configuration = "Release"

[[deploy_targets]]
project = "webpos"
environment = "staging"
iis_path = "/tmp/test/iis"
publish_root = "/tmp/test/publish"
backup_root = "/tmp/test/backup"
deploy_mode = "overlay"
use_app_offline = false
recycle_app_pool_after_deploy = false
"#;
        std::env::set_var("TEST_BOT_TOKEN_5", "token-5");
        let t = write_temp_config(toml);
        let config = RawConfig::from_file(&t.path);
        assert!(config.is_err(), "Should fail on empty main_branch");
        let err = format!("{}", config.err().unwrap());
        assert!(err.contains("main_branch"), "Error: {}", err);
        std::env::remove_var("TEST_BOT_TOKEN_5");
    }

    #[test]
    fn chdir_workspace_path_is_absolute() {
        // We don't require absolute paths, just ensure validation doesn't explode
        let toml = r#"
[app]
name = "Test"
timezone = "UTC"
data_dir = "/tmp/test/data"
log_dir = "/tmp/test/logs"
workspace_root = "/tmp/test/workspace-root"

[telegram]
bot_token_env = "TEST_BOT_TOKEN_6"
allowed_chat_ids = [123]

[[users]]
id = 123
name = "Admin"
role = "admin"

[roles.admin]
can_build = true
can_deploy_staging = true
can_deploy_production = true
can_rollback = true
can_view_logs = true
can_cancel_jobs = true

[tools]
git_path = "git"
msbuild_path = "msbuild"
robocopy_path = "robocopy"
seven_zip_path = "7z"
appcmd_path = "appcmd"

[defaults]
build_timeout_minutes = 30
deploy_timeout_minutes = 15
backup_timeout_minutes = 30
max_log_lines_in_telegram = 80
project_lock_timeout_minutes = 60
keep_staging_on_failure = true
keep_success_staging = false

[[environments]]
key = "staging"
name = "Staging"

[[repositories]]
key = "repo"
name = "Repo"
repo_url = "git@github.com:test/repo.git"
main_branch = "master"
quick_branches = ["master", "develop"]
manual_branch_enabled = true
manual_branch_patterns = ["feature/*", "bugfix/*", "hotfix/*", "release/*", "dev/*"]
forbidden_branch_patterns = ["backup/*"]

[[projects]]
key = "webpos"
name = "WebPOS"
repository = "repo"
repo_url = "git@github.com:test/webpos.git"
workspace = "/tmp/test/workspace"
project_file = "WebPOS.csproj"
configuration = "Release"
main_branch = "master"

[[deploy_targets]]
project = "webpos"
environment = "staging"
iis_path = "/tmp/test/iis"
publish_root = "/tmp/test/publish"
backup_root = "/tmp/test/backup"
deploy_mode = "overlay"
use_app_offline = false
recycle_app_pool_after_deploy = false
"#;
        std::env::set_var("TEST_BOT_TOKEN_6", "token-6");
        let t = write_temp_config(toml);
        let config = RawConfig::from_file(&t.path);
        assert!(config.is_ok(), "Config should be valid: {:?}", config.err());
        std::env::remove_var("TEST_BOT_TOKEN_6");
    }
}
