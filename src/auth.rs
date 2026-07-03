use crate::config::{Config, RolePermissions, UserConfig};

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user: UserConfig,
    pub permissions: RolePermissions,
    pub chat_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Build,
    DeployStaging,
    DeployProduction,
    Rollback,
    ViewLogs,
    CancelJobs,
}

impl AuthContext {
    pub fn has_permission(&self, perm: Permission) -> bool {
        match perm {
            Permission::Build => self.permissions.can_build,
            Permission::DeployStaging => self.permissions.can_deploy_staging,
            Permission::DeployProduction => self.permissions.can_deploy_production,
            Permission::Rollback => self.permissions.can_rollback,
            Permission::ViewLogs => self.permissions.can_view_logs,
            Permission::CancelJobs => self.permissions.can_cancel_jobs,
        }
    }
}

pub fn authenticate(config: &Config, user_id: i64, chat_id: i64) -> Result<AuthContext, AuthError> {
    if !config.telegram.allowed_chat_ids.contains(&chat_id) {
        return Err(AuthError::ChatNotAllowed { chat_id });
    }

    let user = config
        .users
        .iter()
        .find(|u| u.id == user_id)
        .ok_or(AuthError::UserNotFound { user_id })?;

    let permissions =
        config
            .roles
            .get(&user.role)
            .cloned()
            .ok_or_else(|| AuthError::RoleNotFound {
                role: user.role.clone(),
            })?;

    Ok(AuthContext {
        user: user.clone(),
        permissions,
        chat_id,
    })
}

pub fn require_permission(ctx: &AuthContext, permission: Permission) -> Result<(), AuthError> {
    if ctx.has_permission(permission) {
        Ok(())
    } else {
        Err(AuthError::PermissionDenied {
            user_id: ctx.user.id,
            permission,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Chat {chat_id} is not in the allowed list")]
    ChatNotAllowed { chat_id: i64 },

    #[error("User {user_id} not found in config")]
    UserNotFound { user_id: i64 },

    #[error("Role '{role}' not found in config")]
    RoleNotFound { role: String },

    #[error("User {user_id} does not have permission: {permission:?}")]
    PermissionDenied {
        user_id: i64,
        permission: Permission,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn test_config() -> Config {
        let raw = r#"
[app]
name = "Test"
timezone = "UTC"
data_dir = "/tmp/test/data"
log_dir = "/tmp/test/logs"

[telegram]
bot_token_env = "TEST_BOT_TOKEN_AUTH"
allowed_chat_ids = [100, 200]

[[users]]
id = 1
name = "Admin"
role = "admin"

[[users]]
id = 2
name = "Dev"
role = "developer"

[[users]]
id = 3
name = "Viewer"
role = "viewer"

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

[roles.viewer]
can_build = false
can_deploy_staging = false
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

[[projects]]
key = "webpos"
name = "WebPOS"
repo_url = "git@github.com:test/webpos.git"
workspace = "/tmp/test/workspace"
project_file = "/tmp/test/workspace/test.csproj"
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
        let mut file = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        write!(file, "{}", raw).unwrap();
        std::env::set_var("TEST_BOT_TOKEN_AUTH", "test-token-auth");
        let config = Config::from_file(file.path()).unwrap();
        config
    }

    #[test]
    fn admin_has_all_permissions() {
        let config = test_config();
        let ctx = authenticate(&config, 1, 100).unwrap();
        assert!(ctx.has_permission(Permission::Build));
        assert!(ctx.has_permission(Permission::DeployStaging));
        assert!(ctx.has_permission(Permission::DeployProduction));
        assert!(ctx.has_permission(Permission::Rollback));
        assert!(ctx.has_permission(Permission::ViewLogs));
        assert!(ctx.has_permission(Permission::CancelJobs));
    }

    #[test]
    fn developer_lacks_production() {
        let config = test_config();
        let ctx = authenticate(&config, 2, 100).unwrap();
        assert!(ctx.has_permission(Permission::Build));
        assert!(ctx.has_permission(Permission::DeployStaging));
        assert!(!ctx.has_permission(Permission::DeployProduction));
        assert!(!ctx.has_permission(Permission::Rollback));
        assert!(ctx.has_permission(Permission::ViewLogs));
        assert!(!ctx.has_permission(Permission::CancelJobs));
    }

    #[test]
    fn viewer_only_views_logs() {
        let config = test_config();
        let ctx = authenticate(&config, 3, 100).unwrap();
        assert!(!ctx.has_permission(Permission::Build));
        assert!(!ctx.has_permission(Permission::DeployStaging));
        assert!(!ctx.has_permission(Permission::DeployProduction));
        assert!(!ctx.has_permission(Permission::Rollback));
        assert!(ctx.has_permission(Permission::ViewLogs));
        assert!(!ctx.has_permission(Permission::CancelJobs));
    }

    #[test]
    fn chat_not_allowed() {
        let config = test_config();
        let result = authenticate(&config, 1, 999);
        assert!(matches!(
            result,
            Err(AuthError::ChatNotAllowed { chat_id: 999 })
        ));
    }

    #[test]
    fn user_not_found() {
        let config = test_config();
        let result = authenticate(&config, 999, 100);
        assert!(matches!(
            result,
            Err(AuthError::UserNotFound { user_id: 999 })
        ));
    }

    #[test]
    fn require_permission_denied() {
        let config = test_config();
        let ctx = authenticate(&config, 2, 100).unwrap();
        let result = require_permission(&ctx, Permission::DeployProduction);
        assert!(matches!(result, Err(AuthError::PermissionDenied { .. })));
    }

    #[test]
    fn require_permission_allowed() {
        let config = test_config();
        let ctx = authenticate(&config, 2, 100).unwrap();
        let result = require_permission(&ctx, Permission::DeployStaging);
        assert!(result.is_ok());
    }
}
