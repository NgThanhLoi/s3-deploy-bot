use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use teloxide::prelude::*;

use crate::commands::AppState;
use crate::job::{Job, JobStatus};
use crate::session::DeployAction;
use crate::{backup, deploy, git, iis, msbuild, staging};

pub async fn run_job(job_id: String, bot: Bot, state: AppState) -> Result<()> {
    let mut job = state
        .job_store
        .get(&job_id)
        .await
        .ok_or_else(|| anyhow!("Job '{}' not found", job_id))?;

    job.status = JobStatus::Running;
    job.started_at = Some(Utc::now());
    job.stage = "starting".to_string();
    push_log(&mut job, "Job started");
    state.job_store.update(job.clone()).await;
    update_progress_message(&bot, &job).await;

    let result = run_pipeline(&mut job, &bot, &state).await;

    match result {
        Ok(()) => {
            job.status = JobStatus::Success;
            job.stage = "done".to_string();
            job.finished_at = Some(Utc::now());
            push_log(&mut job, "Job completed successfully");
            state.job_store.update(job.clone()).await;
            update_progress_message(&bot, &job).await;
        }
        Err(e) => {
            tracing::error!(
                job_id = %job.job_id,
                project = %job.project_key,
                environment = %job.environment_key,
                branch = %job.branch,
                stage = %job.stage,
                error = ?e,
                "Deploy job failed"
            );
            job.status = JobStatus::Failed;
            job.stage = "failed".to_string();
            job.error = Some(format!("{:#}", e));
            job.finished_at = Some(Utc::now());
            push_log(&mut job, "Job failed");
            cleanup_after_failure(&job, &state).await;
            state.job_store.update(job.clone()).await;
            update_progress_message(&bot, &job).await;
        }
    }

    Ok(())
}

async fn run_pipeline(job: &mut Job, bot: &Bot, state: &AppState) -> Result<()> {
    let project = state
        .config
        .projects
        .iter()
        .find(|p| p.key == job.project_key)
        .cloned()
        .ok_or_else(|| anyhow!("Project '{}' not found", job.project_key))?;
    let repo = state
        .config
        .repositories
        .iter()
        .find(|r| r.key == project.repository)
        .cloned()
        .ok_or_else(|| anyhow!("Repository '{}' not found", project.repository))?;
    let target = state
        .config
        .deploy_targets
        .iter()
        .find(|dt| dt.project == project.key && dt.environment == job.environment_key)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "Deploy target not found for project '{}' environment '{}'",
                project.key,
                job.environment_key
            )
        })?;

    let job_dir = job_dir(&state.config.app.workspace_root, &job.job_id);
    let mirror_dir = mirror_dir(&state.config.app.workspace_root, &repo.key);
    let repo_dir = job_dir.join(format!("{}-worktree", repo.key));
    let build_dir = job_dir.join(format!("{}-build", project.key));

    update_stage(
        job,
        bot,
        state,
        "prepare_workspace",
        "Preparing job workspace",
    )
    .await;
    reset_dir(&job_dir)?;

    update_stage(
        job,
        bot,
        state,
        "git_checkout",
        "Fetching repository cache and creating fresh worktree",
    )
    .await;
    let commit = git::checkout_fresh_worktree(
        &state.config.tools,
        &repo,
        &job.branch,
        &mirror_dir,
        &repo_dir,
    )?;
    job.commit_hash = Some(commit.clone());
    push_log(job, format!("Resolved commit {}", commit));
    state.job_store.update(job.clone()).await;
    update_progress_message(bot, job).await;

    update_stage(
        job,
        bot,
        state,
        "prepare_build_dir",
        "Preparing build directory",
    )
    .await;
    staging::clean_build_dir(&build_dir)?;

    update_stage(
        job,
        bot,
        state,
        "msbuild_publish",
        "Running MSBuild publish",
    )
    .await;
    let build_output = msbuild::publish(&state.config.tools, &project, &repo_dir, &build_dir)?;
    push_log(job, summarize_output("MSBuild", &build_output));
    state.job_store.update(job.clone()).await;
    update_progress_message(bot, job).await;

    update_stage(
        job,
        bot,
        state,
        "cleanup_build",
        "Deleting sensitive build output",
    )
    .await;
    let deleted = staging::delete_entries(&build_dir, &project.delete_from_build)?;
    if deleted.is_empty() {
        push_log(job, "No sensitive build entries matched");
    } else {
        push_log(job, format!("Deleted from build: {}", deleted.join(", ")));
    }
    state.job_store.update(job.clone()).await;
    update_progress_message(bot, job).await;

    if job.action == DeployAction::BuildOnly {
        update_stage(
            job,
            bot,
            state,
            "cleanup_workspace",
            "Cleaning job workspace",
        )
        .await;
        cleanup_job_workspace(&state.config.tools, &mirror_dir, &repo_dir, &job_dir)?;
        return Ok(());
    }

    update_stage(
        job,
        bot,
        state,
        "backup_iis",
        "Backing up current IIS directory",
    )
    .await;
    let backup_path = backup::backup_iis(&project, &target, &job.environment_key)?;
    push_log(job, format!("Backup created: {}", backup_path.display()));
    state.job_store.update(job.clone()).await;
    update_progress_message(bot, job).await;

    update_stage(
        job,
        bot,
        state,
        "deploy_overlay",
        "Copying build output to IIS",
    )
    .await;
    let deploy_output = deploy::copy_overlay(&state.config.tools, &build_dir, &target)?;
    push_log(job, summarize_output("robocopy", &deploy_output));
    state.job_store.update(job.clone()).await;
    update_progress_message(bot, job).await;

    update_stage(job, bot, state, "iis_recycle", "Recycling IIS app pool").await;
    if let Some(output) = iis::recycle_app_pool(&state.config.tools, &target)? {
        push_log(job, summarize_output("appcmd", &output));
    } else {
        push_log(job, "IIS recycle skipped by config");
    }
    state.job_store.update(job.clone()).await;
    update_progress_message(bot, job).await;

    update_stage(
        job,
        bot,
        state,
        "cleanup_workspace",
        "Cleaning job workspace",
    )
    .await;
    cleanup_job_workspace(&state.config.tools, &mirror_dir, &repo_dir, &job_dir)?;
    Ok(())
}

async fn update_stage(job: &mut Job, bot: &Bot, state: &AppState, stage: &str, message: &str) {
    job.stage = stage.to_string();
    push_log(job, message);
    state.job_store.update(job.clone()).await;
    update_progress_message(bot, job).await;
}

async fn update_progress_message(bot: &Bot, job: &Job) {
    let Some(message_id) = job.progress_message_id else {
        return;
    };

    let text = render_progress_text(job);
    if let Err(e) = bot
        .edit_message_text(ChatId(job.chat_id), message_id, text)
        .await
    {
        tracing::warn!(
            "Failed to update progress message for job {}: {:?}",
            job.job_id,
            e
        );
    }
}

fn render_progress_text(job: &Job) -> String {
    let icon = match job.status {
        JobStatus::Queued => "⏳",
        JobStatus::Running => "🔄",
        JobStatus::Success => "✅",
        JobStatus::Failed => "❌",
        JobStatus::Cancelled => "🚫",
    };

    let mut text = format!(
        "{} Job #{}\n\nProject: {}\nEnvironment: {}\nBranch: {}\nAction: {}\nStatus: {}\nStage: {}",
        icon,
        short_id(&job.job_id),
        job.project_key,
        job.environment_key,
        job.branch,
        job.action.label(),
        job.status.label(),
        job.stage
    );

    if let Some(commit) = &job.commit_hash {
        text.push_str(&format!("\nCommit: {}", short_commit(commit)));
    }

    text.push_str("\n\nLog mới nhất:\n");
    for line in job.log.iter().rev().take(8).rev() {
        text.push_str("- ");
        text.push_str(&compact_log_line(line));
        text.push('\n');
    }

    if let Some(error) = &job.error {
        text.push_str("\nLỗi:\n");
        text.push_str(&truncate(error, 900));
    }

    truncate(&text, 3900)
}

async fn cleanup_after_failure(job: &Job, state: &AppState) {
    if state.config.defaults.keep_staging_on_failure {
        return;
    }
    let job_dir = job_dir(&state.config.app.workspace_root, &job.job_id);
    if let Err(e) = cleanup_job_dir(&job_dir) {
        tracing::warn!("Failed to cleanup job dir after failure: {:?}", e);
    }
}

fn mirror_dir(workspace_root: &Path, repo_key: &str) -> PathBuf {
    workspace_root
        .join("repos")
        .join(format!("{}.git", repo_key))
}

fn job_dir(workspace_root: &Path, job_id: &str) -> PathBuf {
    workspace_root.join("jobs").join(job_id)
}

fn reset_dir(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("Failed to remove directory '{}'", path.display()))?;
    }
    std::fs::create_dir_all(path)
        .with_context(|| format!("Failed to create directory '{}'", path.display()))?;
    Ok(())
}

fn cleanup_job_dir(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("Failed to remove job directory '{}'", path.display()))?;
    }
    Ok(())
}

fn cleanup_job_workspace(
    tools: &crate::config::ToolConfig,
    mirror_dir: &Path,
    worktree_dir: &Path,
    job_dir: &Path,
) -> Result<()> {
    if mirror_dir.exists() {
        git::remove_worktree(tools, mirror_dir, worktree_dir)?;
    }
    cleanup_job_dir(job_dir)
}

fn push_log(job: &mut Job, line: impl AsRef<str>) {
    job.log.push(format!(
        "{} {}",
        Utc::now().to_rfc3339(),
        summarize_log_entry(line.as_ref())
    ));
}

fn summarize_output(label: &str, output: &str) -> String {
    format!("{} output:\n{}", label, tail_lines(output, 12))
}

fn summarize_log_entry(value: &str) -> String {
    truncate(&value.replace('\r', "").replace('\n', " | "), 900)
}

fn tail_lines(value: &str, max_lines: usize) -> String {
    let mut lines: Vec<&str> = value.lines().rev().take(max_lines).collect();
    lines.reverse();
    lines.join("\n")
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn short_commit(commit: &str) -> &str {
    commit.get(..8).unwrap_or(commit)
}

fn compact_log_line(line: &str) -> String {
    let without_timestamp = line.split_once(' ').map(|(_, rest)| rest).unwrap_or(line);
    truncate(without_timestamp, 180)
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut output: String = value.chars().take(max_chars.saturating_sub(3)).collect();
    output.push_str("...");
    output
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::Arc;

    use crate::commands::AppState;
    use crate::config::{
        AppConfig, DefaultsConfig, DeployTargetConfig, EnvironmentConfig, ProjectConfig, RawConfig,
        RepositoryConfig, RolePermissions, TelegramConfig, ToolConfig, UserConfig,
    };
    use crate::fast_preset::{store_path, FastPresetStore};
    use crate::job::{Job, JobStatus, JobStore};
    use crate::session::{DeployAction, SessionStore};

    #[tokio::test]
    async fn deploy_pipeline_fake_tools_runs_on_linux() {
        let dir = tempfile::tempdir().unwrap();
        let source_repo = dir.path().join("source-repo");
        let workspace = dir.path().join("workspace");
        let iis_dir = dir.path().join("iis");
        let backup_root = dir.path().join("backup");
        let data_dir = dir.path().join("data");
        let tools_dir = dir.path().join("tools");

        create_source_repo(&source_repo);
        std::fs::create_dir_all(&iis_dir).unwrap();
        std::fs::write(iis_dir.join("old.txt"), "old file").unwrap();
        std::fs::create_dir_all(&tools_dir).unwrap();
        let msbuild = write_executable(
            &tools_dir.join("msbuild_fake.sh"),
            r#"#!/usr/bin/env bash
set -euo pipefail
publish=""
for arg in "$@"; do
  case "$arg" in
    /p:PublishUrl=*) publish="${arg#/p:PublishUrl=}" ;;
  esac
done
if [ -z "$publish" ]; then
  echo "missing PublishUrl" >&2
  exit 2
fi
mkdir -p "$publish/PaymentSetting"
echo "published" > "$publish/index.html"
echo "sensitive" > "$publish/web.config"
echo "secret" > "$publish/PaymentSetting/secret.txt"
echo "fake msbuild published to $publish"
"#,
        );
        let robocopy = write_executable(
            &tools_dir.join("robocopy_fake.sh"),
            r#"#!/usr/bin/env bash
set -euo pipefail
src="$1"
dst="$2"
mkdir -p "$dst"
cp -a "$src"/. "$dst"/
echo "fake robocopy copied $src to $dst"
exit 1
"#,
        );
        let appcmd = write_executable(
            &tools_dir.join("appcmd_fake.sh"),
            r#"#!/usr/bin/env bash
echo "fake appcmd"
"#,
        );

        let config = Arc::new(RawConfig {
            app: AppConfig {
                name: "Test Bot".to_string(),
                timezone: "UTC".to_string(),
                data_dir: data_dir.clone(),
                log_dir: dir.path().join("logs"),
                workspace_root: workspace.clone(),
            },
            telegram: TelegramConfig {
                bot_token_env: "TEST_TOKEN".to_string(),
                allowed_chat_ids: vec![100],
            },
            users: vec![UserConfig {
                id: 1,
                name: "Tester".to_string(),
                role: "admin".to_string(),
            }],
            roles: HashMap::<String, RolePermissions>::new(),
            tools: ToolConfig {
                git_path: PathBuf::from("git"),
                msbuild_path: msbuild,
                robocopy_path: robocopy,
                seven_zip_path: PathBuf::from("7z"),
                appcmd_path: appcmd,
            },
            defaults: DefaultsConfig {
                build_timeout_minutes: 30,
                deploy_timeout_minutes: 15,
                backup_timeout_minutes: 30,
                max_log_lines_in_telegram: 80,
                project_lock_timeout_minutes: 60,
                keep_staging_on_failure: false,
                keep_success_staging: false,
            },
            quick_deploy: None,
            environments: vec![EnvironmentConfig {
                key: "staging".to_string(),
                name: "Staging".to_string(),
                requires_double_confirm: false,
            }],
            repositories: vec![RepositoryConfig {
                key: "s3retail".to_string(),
                name: "S3Retail".to_string(),
                repo_url: source_repo.to_string_lossy().into_owned(),
                main_branch: "master".to_string(),
                quick_branches: vec!["s3-retail-prod".to_string()],
                manual_branch_enabled: true,
                manual_branch_patterns: vec!["*".to_string()],
                forbidden_branch_patterns: vec![],
            }],
            projects: vec![ProjectConfig {
                key: "webpos".to_string(),
                name: "WebPOS".to_string(),
                repository: "s3retail".to_string(),
                project_file: PathBuf::from("Websites/WebPOS/WebPOS.csproj"),
                configuration: "Release".to_string(),
                precompile_before_publish: true,
                enable_updateable: true,
                delete_from_build: vec!["web.config".to_string(), "PaymentSetting".to_string()],
            }],
            deploy_targets: vec![DeployTargetConfig {
                project: "webpos".to_string(),
                environment: "staging".to_string(),
                iis_path: iis_dir.clone(),
                backup_root: backup_root.clone(),
                deploy_mode: "overlay".to_string(),
                use_app_offline: false,
                recycle_app_pool_after_deploy: false,
                app_pool_name: None,
                preserve_files: vec![],
                preserve_dirs: vec![],
            }],
        });
        let job_store = JobStore::new();
        let job = Job::new(
            1,
            100,
            None,
            "webpos".to_string(),
            "staging".to_string(),
            "s3-retail-prod".to_string(),
            DeployAction::BackupAndDeploy,
        );
        let job_id = job.job_id.clone();
        job_store.insert(job).await;
        let state = AppState {
            fast_preset_store: FastPresetStore::new(store_path(&data_dir)),
            config,
            session_store: SessionStore::new(),
            job_store: job_store.clone(),
        };

        run_job(job_id.clone(), Bot::new("123456:test-token"), state)
            .await
            .unwrap();

        let completed = job_store.get(&job_id).await.unwrap();
        assert_eq!(completed.status, JobStatus::Success);
        assert_eq!(completed.stage, "done");
        assert!(completed.commit_hash.is_some());
        assert_eq!(
            std::fs::read_to_string(iis_dir.join("index.html")).unwrap(),
            "published\n"
        );
        assert!(iis_dir.join("old.txt").is_file());
        assert!(!iis_dir.join("web.config").exists());
        assert!(!iis_dir.join("PaymentSetting").exists());
        assert!(workspace.join("repos/s3retail.git").is_dir());
        assert!(!workspace.join("jobs").join(&job_id).exists());
        assert!(backup_root.join("staging").exists());
        assert!(has_zip_file(&backup_root));
    }

    fn create_source_repo(path: &Path) {
        std::fs::create_dir_all(path.join("Websites/WebPOS")).unwrap();
        run_git(path.parent().unwrap(), &["init", path.to_str().unwrap()]);
        run_git(path, &["config", "user.name", "Test User"]);
        run_git(path, &["config", "user.email", "test@example.com"]);
        std::fs::write(path.join("Websites/WebPOS/WebPOS.csproj"), "<Project />").unwrap();
        std::fs::write(path.join("README.md"), "source").unwrap();
        run_git(path, &["add", "."]);
        run_git(path, &["commit", "-m", "initial"]);
        run_git(path, &["checkout", "-b", "s3-retail-prod"]);
        std::fs::write(path.join("branch.txt"), "prod").unwrap();
        run_git(path, &["add", "."]);
        run_git(path, &["commit", "-m", "prod branch"]);
    }

    fn write_executable(path: &Path, content: &str) -> PathBuf {
        std::fs::write(path, content).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
        path.to_path_buf()
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn has_zip_file(path: &Path) -> bool {
        walkdir::WalkDir::new(path)
            .into_iter()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .path()
                    .extension()
                    .map(|ext| ext == "zip")
                    .unwrap_or(false)
            })
    }
}
