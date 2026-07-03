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

    let result = run_pipeline(&mut job, &state).await;

    match result {
        Ok(()) => {
            job.status = JobStatus::Success;
            job.stage = "done".to_string();
            job.finished_at = Some(Utc::now());
            push_log(&mut job, "Job completed successfully");
            state.job_store.update(job.clone()).await;
            bot.send_message(
                ChatId(job.chat_id),
                format!(
                    "✅ Job {} completed\nProject: {}\nEnvironment: {}\nBranch: {}",
                    short_id(&job.job_id),
                    job.project_key,
                    job.environment_key,
                    job.branch
                ),
            )
            .await?;
        }
        Err(e) => {
            job.status = JobStatus::Failed;
            job.stage = "failed".to_string();
            job.error = Some(format!("{:#}", e));
            job.finished_at = Some(Utc::now());
            push_log(&mut job, "Job failed");
            cleanup_after_failure(&job, &state).await;
            state.job_store.update(job.clone()).await;
            bot.send_message(
                ChatId(job.chat_id),
                format!(
                    "❌ Job {} failed\nProject: {}\nEnvironment: {}\nBranch: {}\n\n{}",
                    short_id(&job.job_id),
                    job.project_key,
                    job.environment_key,
                    job.branch,
                    e
                ),
            )
            .await?;
        }
    }

    Ok(())
}

async fn run_pipeline(job: &mut Job, state: &AppState) -> Result<()> {
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
    let repo_dir = job_dir.join(format!("{}-repo", repo.key));
    let build_dir = job_dir.join(format!("{}-build", project.key));

    update_stage(job, state, "prepare_workspace", "Preparing job workspace").await;
    reset_dir(&job_dir)?;

    update_stage(job, state, "git_clone", "Cloning repository").await;
    let commit = git::clone_branch(&state.config.tools, &repo, &job.branch, &repo_dir)?;
    job.commit_hash = Some(commit.clone());
    push_log(job, &format!("Resolved commit {}", commit));
    state.job_store.update(job.clone()).await;

    update_stage(job, state, "prepare_build_dir", "Preparing build directory").await;
    staging::clean_build_dir(&build_dir)?;

    update_stage(job, state, "msbuild_publish", "Running MSBuild publish").await;
    let build_output = msbuild::publish(&state.config.tools, &project, &repo_dir, &build_dir)?;
    push_log(job, summarize_output("MSBuild", &build_output));
    state.job_store.update(job.clone()).await;

    update_stage(
        job,
        state,
        "cleanup_build",
        "Deleting sensitive build output",
    )
    .await;
    let deleted = staging::delete_entries(&build_dir, &project.delete_from_build)?;
    if deleted.is_empty() {
        push_log(job, "No sensitive build entries matched");
    } else {
        push_log(job, &format!("Deleted from build: {}", deleted.join(", ")));
    }
    state.job_store.update(job.clone()).await;

    if job.action == DeployAction::BuildOnly {
        update_stage(job, state, "cleanup_workspace", "Cleaning job workspace").await;
        cleanup_job_dir(&job_dir)?;
        return Ok(());
    }

    update_stage(job, state, "backup_iis", "Backing up current IIS directory").await;
    let backup_path = backup::backup_iis(&project, &target, &job.environment_key)?;
    push_log(job, &format!("Backup created: {}", backup_path.display()));
    state.job_store.update(job.clone()).await;

    update_stage(job, state, "deploy_overlay", "Copying build output to IIS").await;
    let deploy_output = deploy::copy_overlay(&state.config.tools, &build_dir, &target)?;
    push_log(job, summarize_output("robocopy", &deploy_output));
    state.job_store.update(job.clone()).await;

    update_stage(job, state, "iis_recycle", "Recycling IIS app pool").await;
    if let Some(output) = iis::recycle_app_pool(&state.config.tools, &target)? {
        push_log(job, summarize_output("appcmd", &output));
    } else {
        push_log(job, "IIS recycle skipped by config");
    }
    state.job_store.update(job.clone()).await;

    update_stage(job, state, "cleanup_workspace", "Cleaning job workspace").await;
    cleanup_job_dir(&job_dir)?;
    Ok(())
}

async fn update_stage(job: &mut Job, state: &AppState, stage: &str, message: &str) {
    job.stage = stage.to_string();
    push_log(job, message);
    state.job_store.update(job.clone()).await;
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

fn push_log(job: &mut Job, line: &str) {
    job.log
        .push(format!("{} {}", Utc::now().to_rfc3339(), line));
}

fn summarize_output<'a>(label: &str, output: &'a str) -> &'a str {
    let _ = label;
    output
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}
