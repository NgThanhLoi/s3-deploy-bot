use std::sync::Arc;

use teloxide::macros::BotCommands;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId, ParseMode};
use teloxide::utils::command::BotCommands as _;

use crate::auth::{self, AuthContext, Permission};
use crate::config::Config;
use crate::fast_preset::{FastPreset, FastPresetAction};
use crate::job::{Job, JobStore};
use crate::menu;
use crate::runner;
use crate::session::{DeployAction, Session, SessionStep, SessionStore};

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    #[command(description = "Show welcome message")]
    Start,
    #[command(description = "Show your user info")]
    Whoami,
    #[command(description = "Start a new deploy (wizard)")]
    Deploy,
    #[command(description = "Show deploy status")]
    Status,
    #[command(description = "View deploy logs")]
    Log,
    #[command(description = "Cancel current operation")]
    Cancel,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub session_store: SessionStore,
    pub job_store: JobStore,
}

// ---------------------------------------------------------------------------
// Escape helpers for MarkdownV2
// ---------------------------------------------------------------------------

/// Escape special characters for Telegram MarkdownV2.
/// Characters: _ * [ ] ( ) ~ ` > # + - = | { } . !
pub fn escape_md_v2(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|'
            | '{' | '}' | '.' | '!' => {
                let mut buf = String::with_capacity(2);
                buf.push('\\');
                buf.push(c);
                buf
            }
            _ => c.to_string(),
        })
        .collect()
}

/// Send a plain text message (no MarkdownV2 parsing) to avoid escaping issues.
/// If the text contains dynamic data like paths, branches, errors, we use this.
async fn send_plain(bot: &Bot, chat_id: ChatId, text: &str) -> Result<Message, anyhow::Error> {
    Ok(bot.send_message(chat_id, text).await?)
}

/// Send a text message with MarkdownV2 parse mode.
async fn send_md(bot: &Bot, chat_id: ChatId, text: &str) -> Result<Message, anyhow::Error> {
    Ok(bot
        .send_message(chat_id, text)
        .parse_mode(ParseMode::MarkdownV2)
        .await?)
}

async fn edit_plain(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    text: &str,
    reply_markup: Option<InlineKeyboardMarkup>,
) {
    let mut req = bot.edit_message_text(chat_id, msg_id, text);
    if let Some(kb) = reply_markup {
        req = req.reply_markup(kb);
    }
    if let Err(e) = req.await {
        tracing::warn!("edit_message_text (plain) failed: {:?}", e);
    }
}

async fn edit_md(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    text: &str,
    reply_markup: Option<InlineKeyboardMarkup>,
) {
    let mut req = bot.edit_message_text(chat_id, msg_id, text);
    req = req.parse_mode(ParseMode::MarkdownV2);
    if let Some(kb) = reply_markup {
        req = req.reply_markup(kb);
    }
    if let Err(e) = req.await {
        tracing::warn!("edit_message_text (MarkdownV2) failed: {:?}", e);
    }
}

// ---------------------------------------------------------------------------
// /start
// ---------------------------------------------------------------------------

pub async fn handle_start(msg: Message, bot: Bot, state: AppState) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id;
    let user_id = msg.from().map(|u| u.id.0 as i64);

    let reply = match user_id {
        Some(uid) => match auth::authenticate(&state.config, uid, chat_id.0) {
            Ok(ctx) => format!(
                "Xin chào, {}.\n\n\
                 Role: {}\n\
                 Chat ID: {}\n\n\
                 Lệnh dùng nhanh:\n{}",
                ctx.user.name,
                ctx.user.role,
                chat_id.0,
                Command::descriptions()
            ),
            Err(e) => format!(
                "❌ Không có quyền truy cập:\n{}\n\n\
                 User ID ({}) và Chat ID ({}) cần được khai báo trong config.",
                e, uid, chat_id.0
            ),
        },
        None => "❌ Không xác định được Telegram user.".to_string(),
    };

    send_plain(&bot, chat_id, &reply).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// /whoami
// ---------------------------------------------------------------------------

pub async fn handle_whoami(msg: Message, bot: Bot, state: AppState) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id;
    let user_id = msg.from().map(|u| u.id.0 as i64);

    let reply = match user_id {
        Some(uid) => match auth::authenticate(&state.config, uid, chat_id.0) {
            Ok(ctx) => format!(
                "📋 Thông tin tài khoản\n\n\
                 User ID: {}\n\
                 Chat ID: {}\n\
                 Tên: {}\n\
                 Role: {}\n\n\
                 Quyền:\n\
                 * Build: {}\n\
                 * Deploy Staging: {}\n\
                 * Deploy Production: {}\n\
                 * Rollback: {}\n\
                 * View Logs: {}\n\
                 * Cancel Jobs: {}",
                ctx.user.id,
                chat_id.0,
                ctx.user.name,
                ctx.user.role,
                yesno(ctx.permissions.can_build),
                yesno(ctx.permissions.can_deploy_staging),
                yesno(ctx.permissions.can_deploy_production),
                yesno(ctx.permissions.can_rollback),
                yesno(ctx.permissions.can_view_logs),
                yesno(ctx.permissions.can_cancel_jobs),
            ),
            Err(e) => format!(
                "❌ Chưa được cấp quyền:\n{}\n\n\
                 User ID: {}\n\
                 Chat ID: {}",
                e, uid, chat_id.0
            ),
        },
        None => format!(
            "Không xác định được user.\nChat ID: {}\n\n\
             Hãy chat riêng với bot để lấy User ID.",
            chat_id.0
        ),
    };

    send_plain(&bot, chat_id, &reply).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// /deploy
// ---------------------------------------------------------------------------

pub async fn handle_deploy(msg: Message, bot: Bot, state: AppState) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id;

    let ctx = match authenticate_or_reply(&state, bot.clone(), &msg).await {
        Some(ctx) => ctx,
        None => return Ok(()),
    };

    // /deploy only needs can_build to open the wizard
    if let Err(e) = auth::require_permission(&ctx, Permission::Build) {
        send_plain(&bot, chat_id, &format!("❌ {}", e)).await?;
        return Ok(());
    }

    // Check for existing active session
    if let Some(_existing) = state.session_store.find_active_for_chat(chat_id.0).await {
        send_plain(
            &bot,
            chat_id,
            "⚠️ Bạn đang có một phiên deploy chưa kết thúc.\n\
             Dùng /cancel để hủy phiên cũ rồi chạy /deploy lại.",
        )
        .await?;
        return Ok(());
    }

    let session = state.session_store.create(ctx.user.id, chat_id.0).await;

    let text = "🚀 *Deploy Wizard*\n\n*Bước 1/5:* Chọn môi trường";
    let keyboard = menu::environment_keyboard(&state.config);

    let sent = bot
        .send_message(chat_id, text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;

    let mut session = session;
    session.message_id = Some(sent.id);
    state.session_store.update(session).await;

    Ok(())
}

// ---------------------------------------------------------------------------
// /status
// ---------------------------------------------------------------------------

pub async fn handle_status(msg: Message, bot: Bot, state: AppState) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id;

    let ctx = match authenticate_or_reply(&state, bot.clone(), &msg).await {
        Some(ctx) => ctx,
        None => return Ok(()),
    };

    if let Err(e) = auth::require_permission(&ctx, Permission::ViewLogs) {
        send_plain(&bot, chat_id, &format!("❌ {}", e)).await?;
        return Ok(());
    }

    let jobs = state.job_store.recent_for_chat(chat_id.0, 5).await;
    if jobs.is_empty() {
        send_plain(&bot, chat_id, "📊 Trạng thái\n\nChưa có job nào.").await?;
    } else {
        let mut text = String::from("📊 Job gần đây\n\n");
        for job in jobs {
            text.push_str(&format!(
                "#{} - {}\nProject: {} / {}\nBranch: {}\nStage: {}\n\n",
                short_id(&job.job_id),
                job.status.label(),
                job.project_key,
                job.environment_key,
                job.branch,
                job.stage
            ));
        }
        send_plain(&bot, chat_id, &text).await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// /log
// ---------------------------------------------------------------------------

pub async fn handle_log(msg: Message, bot: Bot, state: AppState) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id;

    let ctx = match authenticate_or_reply(&state, bot.clone(), &msg).await {
        Some(ctx) => ctx,
        None => return Ok(()),
    };

    if let Err(e) = auth::require_permission(&ctx, Permission::ViewLogs) {
        send_plain(&bot, chat_id, &format!("❌ {}", e)).await?;
        return Ok(());
    }

    let jobs = state.job_store.recent_for_chat(chat_id.0, 1).await;
    if let Some(job) = jobs.first() {
        let text = render_log_message(job, state.config.defaults.max_log_lines_in_telegram);
        send_plain(&bot, chat_id, &text).await?;
    } else {
        send_plain(&bot, chat_id, "📋 Log\n\nChưa có job nào.").await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// /cancel
// ---------------------------------------------------------------------------

pub async fn handle_cancel(msg: Message, bot: Bot, state: AppState) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id;
    let user_id = msg.from().map(|u| u.id.0 as i64);

    if let Some(uid) = user_id {
        if let Some(session) = state
            .session_store
            .find_by_chat_and_user(chat_id.0, uid)
            .await
        {
            state.session_store.remove(&session.session_id).await;
            edit_session_message(
                &bot,
                chat_id,
                session.message_id,
                "❌ Deploy cancelled",
                None,
            )
            .await;
        }
    }

    send_plain(&bot, chat_id, "✅ Deploy cancelled.").await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Callback query handler (inline keyboard buttons)
// ---------------------------------------------------------------------------

pub async fn handle_callback(
    q: CallbackQuery,
    bot: Bot,
    state: AppState,
) -> Result<(), anyhow::Error> {
    let callback_id = q.id.clone();
    let data = q.data.unwrap_or_default();
    let user_id = q.from.id.0 as i64;

    // Get or find session from message
    let chat_id = match &q.message {
        Some(msg) => msg.chat.id,
        None => {
            bot.answer_callback_query(callback_id).await?;
            return Ok(());
        }
    };

    let mut session = match state
        .session_store
        .find_by_chat_and_user(chat_id.0, user_id)
        .await
    {
        Some(s) => s,
        None => {
            bot.answer_callback_query(&callback_id)
                .text("Không có phiên deploy. Dùng /deploy để bắt đầu.")
                .await?;
            return Ok(());
        }
    };

    // Only the session owner can press buttons
    if session.owner_user_id != user_id {
        bot.answer_callback_query(&callback_id)
            .text("Phiên deploy này thuộc user khác.")
            .await?;
        return Ok(());
    }

    // Handle navigation
    match data.as_str() {
        "nav:cancel" => {
            state.session_store.remove(&session.session_id).await;
            edit_session_message(&bot, chat_id, session.message_id, "❌ Đã hủy deploy", None).await;
            bot.answer_callback_query(callback_id).await?;
            return Ok(());
        }
        "nav:back" => {
            let prev = previous_step(session.step);
            session.set_step(prev);
            state.session_store.update(session.clone()).await;
            bot.answer_callback_query(callback_id).await?;
            show_step(&session, &state, &bot, chat_id).await?;
            return Ok(());
        }
        _ => {}
    }

    match session.step {
        SessionStep::SelectEnvironment => {
            if data == "quick:deploy" {
                handle_quick_deploy(&mut session, &state, &bot, chat_id, &callback_id).await?
            } else {
                handle_env_selected(&mut session, &state, &bot, chat_id, &callback_id, &data)
                    .await?
            }
        }
        SessionStep::SelectProject => {
            handle_project_selected(&mut session, &state, &bot, chat_id, &callback_id, &data)
                .await?
        }
        SessionStep::SelectBranch => {
            handle_branch_selected(&mut session, &state, &bot, chat_id, &callback_id, &data).await?
        }
        SessionStep::SelectAction => {
            handle_action_selected(&mut session, &state, &bot, chat_id, &callback_id, &data).await?
        }
        SessionStep::Confirm | SessionStep::DoubleConfirm => {
            handle_confirm(&mut session, &state, &bot, chat_id, &callback_id, &data).await?
        }
        _ => {
            bot.answer_callback_query(&callback_id)
                .text("Phiên deploy đang lỗi trạng thái. Dùng /cancel để reset.")
                .await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Text message handler (for manual branch input)
// ---------------------------------------------------------------------------

pub async fn handle_text_message(
    msg: Message,
    bot: Bot,
    state: AppState,
) -> Result<(), anyhow::Error> {
    let chat_id = msg.chat.id;
    let user_id = match msg.from().map(|u| u.id.0 as i64) {
        Some(uid) => uid,
        None => return Ok(()),
    };

    let raw_text = match msg.text() {
        Some(t) => t,
        None => return Ok(()),
    };

    let mut session = match state
        .session_store
        .find_by_chat_and_user(chat_id.0, user_id)
        .await
    {
        Some(s) => s,
        None => return Ok(()), // No active session, ignore message
    };

    if session.step != SessionStep::WaitingManualBranch {
        return Ok(()); // Not waiting for input
    }

    let repo = session_repository(&session, &state.config);

    match validate_manual_branch(raw_text, repo) {
        Ok(branch) => {
            session.branch = Some(branch);
            session.set_step(SessionStep::SelectAction);
            state.session_store.update(session.clone()).await;

            show_step(&session, &state, &bot, chat_id).await?;
        }
        Err(e) => {
            send_plain(&bot, chat_id, &format!("❌ Branch không hợp lệ: {}", e)).await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Branch validation
// ---------------------------------------------------------------------------

/// Validate a manual branch name according to repository rules.
/// Returns the trimmed branch name on success, or an error message on failure.
pub fn validate_manual_branch(
    input: &str,
    repository: Option<&crate::config::RepositoryConfig>,
) -> Result<String, String> {
    let trimmed = input.trim();

    // Not empty
    if trimmed.is_empty() {
        return Err("Branch name cannot be empty.".to_string());
    }

    // Max length 120
    if trimmed.len() > 120 {
        return Err("Branch name must be at most 120 characters.".to_string());
    }

    // No whitespace
    if trimmed.contains(char::is_whitespace) {
        return Err("Branch name must not contain whitespace.".to_string());
    }

    // Forbidden characters: ; & | > < " '
    let forbidden_chars = [';', '&', '|', '>', '<', '"', '\''];
    if let Some(c) = trimmed.chars().find(|c| forbidden_chars.contains(c)) {
        return Err(format!("Branch name must not contain character '{}'.", c));
    }

    // No `..`
    if trimmed.contains("..") {
        return Err("Branch name must not contain '..'.".to_string());
    }

    // No backslash
    if trimmed.contains('\\') {
        return Err("Branch name must not contain backslash.".to_string());
    }

    // Must not start with /
    if trimmed.starts_with('/') {
        return Err("Branch name must not start with '/'.".to_string());
    }

    // Must not end with /
    if trimmed.ends_with('/') {
        return Err("Branch name must not end with '/'.".to_string());
    }

    // No //
    if trimmed.contains("//") {
        return Err("Branch name must not contain '//'.".to_string());
    }

    if let Some(repo) = repository {
        // Must not match forbidden_branch_patterns
        if !repo.forbidden_branch_patterns.is_empty() {
            let forbidden = repo
                .forbidden_branch_patterns
                .iter()
                .any(|pat| glob_match(pat, trimmed));
            if forbidden {
                return Err(format!(
                    "Branch '{}' matches a forbidden pattern: {:?}",
                    trimmed, repo.forbidden_branch_patterns
                ));
            }
        }

        // Must match at least one manual_branch_patterns
        if !repo.manual_branch_patterns.is_empty() {
            let matched = repo
                .manual_branch_patterns
                .iter()
                .any(|pat| glob_match(pat, trimmed));
            if !matched {
                return Err(format!(
                    "Branch '{}' does not match any allowed pattern: {:?}",
                    trimmed, repo.manual_branch_patterns
                ));
            }
        }
    }

    Ok(trimmed.to_string())
}

/// Simple glob matching using the globset crate.
fn glob_match(pattern: &str, value: &str) -> bool {
    let matcher = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build();
    match matcher {
        Ok(m) => m.compile_matcher().is_match(value),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Permission check helpers (action-specific)
// ---------------------------------------------------------------------------

/// Check permission for a given action/environment combination.
/// Returns Ok if allowed, Err with message if denied.
fn check_action_permission(
    ctx: &AuthContext,
    action: DeployAction,
    env_key: &str,
    config: &Config,
) -> Result<(), String> {
    match action {
        DeployAction::BuildOnly => {
            // Build only always needs can_build
            auth::require_permission(ctx, Permission::Build).map_err(|e| format!("{}", e))
        }
        DeployAction::BackupAndDeploy => {
            let env = config.environments.iter().find(|e| e.key == env_key);
            match env.map(|e| e.key.as_str()) {
                Some("production") => auth::require_permission(ctx, Permission::DeployProduction)
                    .map_err(|e| format!("{}", e)),
                _ => auth::require_permission(ctx, Permission::DeployStaging)
                    .map_err(|e| format!("{}", e)),
            }
        }
    }
}

fn validate_preset_for_user(
    ctx: &AuthContext,
    config: &Config,
    preset: &FastPreset,
) -> Result<DeployAction, String> {
    let action = match preset.action {
        FastPresetAction::Build => DeployAction::BuildOnly,
        FastPresetAction::Deploy => DeployAction::BackupAndDeploy,
    };

    let project = config
        .projects
        .iter()
        .find(|project| project.key == preset.project)
        .ok_or_else(|| format!("Project '{}' not found.", preset.project))?;

    if !config
        .environments
        .iter()
        .any(|environment| environment.key == preset.environment)
    {
        return Err(format!("Environment '{}' not found.", preset.environment));
    }

    if !config
        .deploy_targets
        .iter()
        .any(|target| target.project == preset.project && target.environment == preset.environment)
    {
        return Err(format!(
            "Preset '{}' has no deploy target for project '{}' environment '{}'.",
            preset.name, preset.project, preset.environment
        ));
    }

    let repo = config
        .repositories
        .iter()
        .find(|repo| repo.key == project.repository)
        .ok_or_else(|| format!("Repository '{}' not found.", project.repository))?;

    let quick_branch =
        preset.branch == repo.main_branch || repo.quick_branches.iter().any(|b| b == &preset.branch);
    if !quick_branch {
        if !repo.manual_branch_enabled {
            return Err(format!(
                "Branch '{}' is not configured for repository '{}'.",
                preset.branch, repo.key
            ));
        }
        validate_manual_branch(&preset.branch, Some(repo))
            .map_err(|e| format!("Branch '{}' is invalid: {}", preset.branch, e))?;
    }

    check_action_permission(ctx, action, &preset.environment, config)
        .map_err(|e| format!("Permission denied: {}", e))?;

    Ok(action)
}

// ---------------------------------------------------------------------------
// Step handlers
// ---------------------------------------------------------------------------

async fn handle_env_selected(
    session: &mut Session,
    state: &AppState,
    bot: &Bot,
    chat_id: ChatId,
    callback_id: &str,
    data: &str,
) -> Result<(), anyhow::Error> {
    let env_key = data.strip_prefix("env:").unwrap_or("");
    if env_key.is_empty() {
        return Ok(());
    }

    // Validate environment exists
    let env = state.config.environments.iter().find(|e| e.key == env_key);
    let env_exists = env.is_some();
    if !env_exists {
        bot.answer_callback_query(callback_id)
            .text("Không tìm thấy môi trường.")
            .await?;
        return Ok(());
    }

    session.environment_key = Some(env_key.to_string());
    session.set_step(SessionStep::SelectProject);
    state.session_store.update(session.clone()).await;

    bot.answer_callback_query(callback_id).await?;
    show_step(session, state, bot, chat_id).await?;
    Ok(())
}

async fn handle_quick_deploy(
    session: &mut Session,
    state: &AppState,
    bot: &Bot,
    chat_id: ChatId,
    callback_id: &str,
) -> Result<(), anyhow::Error> {
    let quick = match &state.config.quick_deploy {
        Some(quick) if quick.enabled => quick,
        _ => {
            bot.answer_callback_query(callback_id)
                .text("Fast deploy chưa được cấu hình.")
                .await?;
            return Ok(());
        }
    };

    let action = match quick.action.as_str() {
        "build" => DeployAction::BuildOnly,
        "deploy" => DeployAction::BackupAndDeploy,
        _ => {
            bot.answer_callback_query(callback_id)
                .text("quick_deploy.action không hợp lệ.")
                .await?;
            return Ok(());
        }
    };

    let has_target = state
        .config
        .deploy_targets
        .iter()
        .any(|dt| dt.project == quick.project && dt.environment == quick.environment);
    if !has_target {
        bot.answer_callback_query(callback_id)
            .text("Fast deploy chưa có deploy target hợp lệ.")
            .await?;
        return Ok(());
    }

    let ctx_result = auth::authenticate(&state.config, session.owner_user_id, chat_id.0);
    match ctx_result {
        Ok(ctx) => {
            if let Err(e) = check_action_permission(&ctx, action, &quick.environment, &state.config)
            {
                bot.answer_callback_query(callback_id)
                    .text(format!("Permission denied: {}", e))
                    .await?;
                return Ok(());
            }
        }
        Err(e) => {
            bot.answer_callback_query(callback_id)
                .text(format!("Auth error: {}", e))
                .await?;
            return Ok(());
        }
    }

    session.environment_key = Some(quick.environment.clone());
    session.project_key = Some(quick.project.clone());
    session.branch = Some(quick.branch.clone());
    session.action = Some(action);
    session.set_step(SessionStep::Confirm);
    state.session_store.update(session.clone()).await;

    bot.answer_callback_query(callback_id)
        .text("Đã chọn cấu hình fast deploy.")
        .await?;
    show_step(session, state, bot, chat_id).await?;
    Ok(())
}

async fn handle_project_selected(
    session: &mut Session,
    state: &AppState,
    bot: &Bot,
    chat_id: ChatId,
    callback_id: &str,
    data: &str,
) -> Result<(), anyhow::Error> {
    let proj_key = data.strip_prefix("project:").unwrap_or("");
    if proj_key.is_empty() {
        return Ok(());
    }

    let env_key = match &session.environment_key {
        Some(k) => k,
        None => {
            bot.answer_callback_query(callback_id)
                .text("Chưa chọn môi trường.")
                .await?;
            return Ok(());
        }
    };

    // Validate project exists
    let proj = state.config.projects.iter().find(|p| p.key == proj_key);
    if proj.is_none() {
        bot.answer_callback_query(callback_id)
            .text("Không tìm thấy project.")
            .await?;
        return Ok(());
    }

    // Validate project has deploy target for this environment
    let has_target = state
        .config
        .deploy_targets
        .iter()
        .any(|dt| dt.project == proj_key && dt.environment == *env_key);
    if !has_target {
        bot.answer_callback_query(callback_id)
            .text("Project này chưa có target cho môi trường đã chọn.")
            .await?;
        return Ok(());
    }

    session.project_key = Some(proj_key.to_string());
    session.set_step(SessionStep::SelectBranch);
    state.session_store.update(session.clone()).await;

    bot.answer_callback_query(callback_id).await?;
    show_step(session, state, bot, chat_id).await?;
    Ok(())
}

async fn handle_branch_selected(
    session: &mut Session,
    state: &AppState,
    bot: &Bot,
    chat_id: ChatId,
    callback_id: &str,
    data: &str,
) -> Result<(), anyhow::Error> {
    let proj = session
        .project_key
        .as_ref()
        .and_then(|k| state.config.projects.iter().find(|p| p.key == *k));
    let repo = proj.and_then(|p| {
        state
            .config
            .repositories
            .iter()
            .find(|r| r.key == p.repository)
    });

    match data {
        "branch:manual" => {
            let msg = match (proj, repo) {
                (Some(p), Some(r)) => {
                    let examples: Vec<&str> = r
                        .manual_branch_patterns
                        .iter()
                        .take(3)
                        .map(|s| s.as_str())
                        .collect();
                    format!(
                        "✍️ Enter branch name for {}\n\n\
                         Examples:\n{}\n\n\
                         Cancel with /cancel.",
                        p.name,
                        examples.join("\n")
                    )
                }
                _ => "✍️ Enter branch name.".to_string(),
            };

            session.set_step(SessionStep::WaitingManualBranch);
            state.session_store.update(session.clone()).await;

            bot.answer_callback_query(callback_id).await?;
            edit_session_message(bot, chat_id, session.message_id, &msg, None).await;
            Ok(())
        }
        _ => {
            let branch = data.strip_prefix("branch:").unwrap_or("");
            if branch.is_empty() {
                return Ok(());
            }

            // Validate branch is main_branch or in quick_branches
            let valid = match repo {
                Some(r) => branch == r.main_branch || r.quick_branches.iter().any(|b| b == branch),
                None => false,
            };
            if !valid {
                bot.answer_callback_query(callback_id)
                    .text("Invalid branch selection.")
                    .await?;
                return Ok(());
            }

            session.branch = Some(branch.to_string());
            session.set_step(SessionStep::SelectAction);
            state.session_store.update(session.clone()).await;

            bot.answer_callback_query(callback_id).await?;
            show_step(session, state, bot, chat_id).await?;
            Ok(())
        }
    }
}

async fn handle_action_selected(
    session: &mut Session,
    state: &AppState,
    bot: &Bot,
    chat_id: ChatId,
    callback_id: &str,
    data: &str,
) -> Result<(), anyhow::Error> {
    let action = match data {
        "action:build" => DeployAction::BuildOnly,
        "action:deploy" => DeployAction::BackupAndDeploy,
        _ => {
            bot.answer_callback_query(callback_id)
                .text("Invalid action.")
                .await?;
            return Ok(());
        }
    };

    // Validate deploy target exists for project + environment
    let env_key = session.environment_key.as_deref().unwrap_or("");
    let proj_key = session.project_key.as_deref().unwrap_or("");
    let has_target = state
        .config
        .deploy_targets
        .iter()
        .any(|dt| dt.project == proj_key && dt.environment == env_key);
    if !has_target {
        bot.answer_callback_query(callback_id)
            .text("No deploy target for this combination.")
            .await?;
        return Ok(());
    }

    // Store action temporarily to check permission
    let saved_action = session.action;
    session.action = Some(action);

    // Check permission before proceeding to confirm
    // We need to get auth context for the session owner
    let ctx_result = auth::authenticate(&state.config, session.owner_user_id, chat_id.0);
    match ctx_result {
        Ok(ctx) => {
            if let Err(e) = check_action_permission(&ctx, action, env_key, &state.config) {
                session.action = saved_action; // restore
                bot.answer_callback_query(callback_id)
                    .text(format!("Permission denied: {}", e))
                    .await?;
                return Ok(());
            }
        }
        Err(e) => {
            session.action = saved_action; // restore
            bot.answer_callback_query(callback_id)
                .text(format!("Auth error: {}", e))
                .await?;
            return Ok(());
        }
    }

    session.set_step(SessionStep::Confirm);
    state.session_store.update(session.clone()).await;

    bot.answer_callback_query(callback_id).await?;
    show_step(session, state, bot, chat_id).await?;
    Ok(())
}

async fn handle_confirm(
    session: &mut Session,
    state: &AppState,
    bot: &Bot,
    chat_id: ChatId,
    callback_id: &str,
    data: &str,
) -> Result<(), anyhow::Error> {
    match data {
        "confirm:yes" => {
            let env_key = session.environment_key.as_deref().unwrap_or("");
            let env = state.config.environments.iter().find(|e| e.key == env_key);

            let needs_double = env.map(|e| e.requires_double_confirm).unwrap_or(false);

            if needs_double && session.step == SessionStep::Confirm {
                session.set_step(SessionStep::DoubleConfirm);
                state.session_store.update(session.clone()).await;
                bot.answer_callback_query(callback_id).await?;
                show_step(session, state, bot, chat_id).await
            } else {
                let project_key = match session.project_key.clone() {
                    Some(v) => v,
                    None => {
                        bot.answer_callback_query(callback_id)
                            .text("Missing project.")
                            .await?;
                        return Ok(());
                    }
                };
                let environment_key = match session.environment_key.clone() {
                    Some(v) => v,
                    None => {
                        bot.answer_callback_query(callback_id)
                            .text("Missing environment.")
                            .await?;
                        return Ok(());
                    }
                };
                let branch = match session.branch.clone() {
                    Some(v) => v,
                    None => {
                        bot.answer_callback_query(callback_id)
                            .text("Missing branch.")
                            .await?;
                        return Ok(());
                    }
                };
                let action = match session.action {
                    Some(v) => v,
                    None => {
                        bot.answer_callback_query(callback_id)
                            .text("Missing action.")
                            .await?;
                        return Ok(());
                    }
                };

                if state
                    .job_store
                    .has_running_target(&project_key, &environment_key)
                    .await
                {
                    bot.answer_callback_query(callback_id)
                        .text("This project/environment already has a queued or running job.")
                        .await?;
                    return Ok(());
                }

                let job = Job::new(
                    session.owner_user_id,
                    chat_id.0,
                    session.message_id,
                    project_key,
                    environment_key,
                    branch,
                    action,
                );
                let job_id = job.job_id.clone();
                state.job_store.insert(job).await;

                session.set_step(SessionStep::Done);
                state.session_store.update(session.clone()).await;

                bot.answer_callback_query(callback_id)
                    .text("Deploy queued!")
                    .await?;
                let report = build_complete_text(session, state, &job_id);
                edit_session_message(bot, chat_id, session.message_id, &report, None).await;

                let runner_state = state.clone();
                let runner_bot = bot.clone();
                tokio::spawn(async move {
                    if let Err(e) = runner::run_job(job_id, runner_bot, runner_state).await {
                        tracing::error!("Job runner failed: {:?}", e);
                    }
                });
                Ok(())
            }
        }
        "confirm:no" => {
            state.session_store.remove(&session.session_id).await;
            bot.answer_callback_query(callback_id)
                .text("Cancelled")
                .await?;
            edit_session_message(
                bot,
                chat_id,
                session.message_id,
                "❌ Deploy cancelled",
                None,
            )
            .await;
            Ok(())
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

async fn show_step(
    session: &Session,
    state: &AppState,
    bot: &Bot,
    chat_id: ChatId,
) -> Result<(), anyhow::Error> {
    let (text, keyboard) = build_step_content(session, state);
    edit_session_message(bot, chat_id, session.message_id, &text, Some(keyboard)).await;
    Ok(())
}

fn build_step_content(session: &Session, state: &AppState) -> (String, InlineKeyboardMarkup) {
    match session.step {
        SessionStep::SelectEnvironment => {
            let text = "🚀 *Deploy Wizard*\n\n*Bước 1/5:* Chọn môi trường".to_string();
            let keyboard = menu::environment_keyboard(&state.config);
            (text, keyboard)
        }
        SessionStep::SelectProject => {
            let env_name = session
                .environment_key
                .as_ref()
                .and_then(|k| state.config.environments.iter().find(|e| e.key == *k))
                .map(|e| &e.name[..])
                .unwrap_or("?");

            let text = format!(
                "🚀 *Deploy Wizard*\n\n*Môi trường:* {}\n\n*Bước 2/5:* Chọn project",
                escape_md_v2(env_name)
            );

            let env_key = session.environment_key.as_deref().unwrap_or("");
            let keyboard = menu::project_keyboard(&state.config, env_key);
            (text, keyboard)
        }
        SessionStep::SelectBranch => {
            let proj = session_project(session, &state.config);
            let repo = session_repository(session, &state.config);

            let proj_name = proj.map(|p| &p.name[..]).unwrap_or("?");
            let env_name = selected_env_name(session, &state.config);
            let repo_name = repo.map(|r| &r.name[..]).unwrap_or("?");

            let text = format!(
                "🚀 *Deploy Wizard*\n\n*Môi trường:* {}\n*Project:* {}\n*Repo:* {}\n\n*Bước 3/5:* Chọn branch",
                escape_md_v2(env_name),
                escape_md_v2(proj_name),
                escape_md_v2(repo_name)
            );

            let (main_branch, quick, manual) = repo
                .map(|r| {
                    (
                        r.main_branch.as_str(),
                        r.quick_branches.clone(),
                        r.manual_branch_enabled,
                    )
                })
                .unwrap_or(("master", vec![], false));
            let keyboard = menu::branch_keyboard(main_branch, &quick, manual);
            (text, keyboard)
        }
        SessionStep::WaitingManualBranch => {
            let proj_name = session_project(session, &state.config)
                .map(|p| &p.name[..])
                .unwrap_or("?");
            let repo = session_repository(session, &state.config);
            let patterns = repo
                .map(|r| {
                    if r.manual_branch_patterns.is_empty() {
                        "Không giới hạn pattern".to_string()
                    } else {
                        r.manual_branch_patterns.join(", ")
                    }
                })
                .unwrap_or_else(|| "Không tìm thấy cấu hình repo".to_string());

            let text = format!(
                "✍️ *Nhập branch cho {}*\n\n\
                 Pattern hợp lệ: `{}`\n\n\
                 Gửi tên branch bằng tin nhắn text\\. Dùng /cancel để hủy\\.",
                escape_md_v2(proj_name),
                escape_md_v2(&patterns)
            );

            (text, InlineKeyboardMarkup::default())
        }
        SessionStep::SelectAction => {
            let env_name = selected_env_name(session, &state.config);
            let proj_name = session_project(session, &state.config)
                .map(|p| &p.name[..])
                .unwrap_or("?");
            let branch = session.branch.as_deref().unwrap_or("?");
            let text = format!(
                "🚀 *Deploy Wizard*\n\n*Môi trường:* {}\n*Project:* {}\n*Branch:* `{}`\n\n*Bước 4/5:* Chọn thao tác",
                escape_md_v2(env_name),
                escape_md_v2(proj_name),
                escape_md_v2(branch)
            );
            let keyboard = menu::action_keyboard();
            (text, keyboard)
        }
        SessionStep::Confirm => {
            let text = build_summary_text(session, state, false);
            let keyboard = menu::confirm_keyboard(false);
            (text, keyboard)
        }
        SessionStep::DoubleConfirm => {
            let text = build_summary_text(session, state, true);
            let keyboard = menu::confirm_keyboard(true);
            (text, keyboard)
        }
        SessionStep::FastPresetList
        | SessionStep::FastPresetManageList
        | SessionStep::FastPresetManageOne
        | SessionStep::FastPresetCreateName
        | SessionStep::FastPresetEditField
        | SessionStep::FastPresetDeleteConfirm => {
            let text = "⚡ *Fast Deploy*\n\nTính năng đang chuẩn bị\\.".to_string();
            (text, InlineKeyboardMarkup::default())
        }
        SessionStep::Done => {
            let text =
                "✅ *Job đã được tạo*\n\nDùng /status hoặc /log để xem tiến trình\\.".to_string();
            (text, InlineKeyboardMarkup::default())
        }
    }
}

fn build_summary_text(session: &Session, state: &AppState, is_double: bool) -> String {
    let env_key = session.environment_key.as_deref().unwrap_or("?");
    let env_name = state
        .config
        .environments
        .iter()
        .find(|e| e.key == env_key)
        .map(|e| &e.name[..])
        .unwrap_or("?");

    let proj_key = session.project_key.as_deref().unwrap_or("?");
    let proj_name = state
        .config
        .projects
        .iter()
        .find(|p| p.key == proj_key)
        .map(|p| &p.name[..])
        .unwrap_or("?");

    let branch = session.branch.as_deref().unwrap_or("?");
    let commit = session.commit_hash.as_deref().unwrap_or("will resolve");

    let action_label = session
        .action
        .map(|a| a.label().to_string())
        .unwrap_or_else(|| "?".to_string());

    let target = state
        .config
        .deploy_targets
        .iter()
        .find(|dt| dt.project == proj_key && dt.environment == env_key)
        .map(|dt| format!("{}", dt.iis_path.display()))
        .unwrap_or_else(|| "N/A".to_string());

    if is_double {
        format!(
            "🔴 *Xác nhận Production lần cuối*\n\n\
             Môi trường: {}\n\
             Project: {}\n\
             Branch: `{}`\n\
             Thao tác: {}\n\
             IIS path: {}\n\n\
             Bot sẽ build vào workspace tạm, xóa file nhạy cảm, backup IIS hiện tại rồi copy đè vào IIS\\.",
            escape_md_v2(env_name),
            escape_md_v2(proj_name),
            escape_md_v2(branch),
            escape_md_v2(&action_label),
            escape_md_v2(&target)
        )
    } else {
        format!(
            "⚠️ *Kiểm tra trước khi chạy*\n\n\
             *Môi trường:* {}\n\
             *Project:* {}\n\
             *Branch:* `{}`\n\
             *Commit:* `{}`\n\
             *Thao tác:* {}\n\
             *IIS path:* {}\n\n\
             Flow: clone branch → MSBuild publish → xóa file nhạy cảm → backup IIS → copy overlay \\(`/E`, không `/MIR`\\){}",
            escape_md_v2(env_name),
            escape_md_v2(proj_name),
            escape_md_v2(branch),
            escape_md_v2(commit),
            escape_md_v2(&action_label),
            escape_md_v2(&target),
            if session.action == Some(DeployAction::BuildOnly) {
                "\n\nBuild only sẽ dừng sau bước publish và cleanup build output\\."
            } else {
                ""
            }
        )
    }
}

fn build_complete_text(session: &Session, state: &AppState, job_id: &str) -> String {
    let env_key = session.environment_key.as_deref().unwrap_or("?");
    let env_name = state
        .config
        .environments
        .iter()
        .find(|e| e.key == env_key)
        .map(|e| &e.name[..])
        .unwrap_or("?");

    let proj_key = session.project_key.as_deref().unwrap_or("?");
    let proj_name = state
        .config
        .projects
        .iter()
        .find(|p| p.key == proj_key)
        .map(|p| &p.name[..])
        .unwrap_or("?");

    let branch = session.branch.as_deref().unwrap_or("?");
    let action_label = session
        .action
        .map(|a| a.label().to_string())
        .unwrap_or_else(|| "?".to_string());

    format!(
        "✅ Job đã được tạo\n\n\
         *Job:* `{}`\n\
         *Project:* {}\n\
         *Môi trường:* {}\n\
         *Branch:* `{}`\n\
         *Thao tác:* {}\n\n\
         Dùng /status hoặc /log để xem tiến trình\\.",
        escape_md_v2(short_id(job_id)),
        escape_md_v2(proj_name),
        escape_md_v2(env_name),
        escape_md_v2(branch),
        escape_md_v2(&action_label)
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn previous_step(current: SessionStep) -> SessionStep {
    match current {
        SessionStep::SelectEnvironment => SessionStep::SelectEnvironment,
        SessionStep::SelectProject => SessionStep::SelectEnvironment,
        SessionStep::SelectBranch => SessionStep::SelectProject,
        SessionStep::WaitingManualBranch => SessionStep::SelectBranch,
        SessionStep::SelectAction => SessionStep::SelectBranch,
        SessionStep::Confirm => SessionStep::SelectAction,
        SessionStep::DoubleConfirm => SessionStep::Confirm,
        SessionStep::FastPresetList => SessionStep::SelectEnvironment,
        SessionStep::FastPresetManageList => SessionStep::FastPresetList,
        SessionStep::FastPresetManageOne => SessionStep::FastPresetManageList,
        SessionStep::FastPresetCreateName => SessionStep::FastPresetList,
        SessionStep::FastPresetEditField => SessionStep::FastPresetManageOne,
        SessionStep::FastPresetDeleteConfirm => SessionStep::FastPresetManageOne,
        SessionStep::Done => SessionStep::Done,
    }
}

fn session_project<'a>(
    session: &Session,
    config: &'a Config,
) -> Option<&'a crate::config::ProjectConfig> {
    session
        .project_key
        .as_ref()
        .and_then(|k| config.projects.iter().find(|p| p.key == *k))
}

fn session_repository<'a>(
    session: &Session,
    config: &'a Config,
) -> Option<&'a crate::config::RepositoryConfig> {
    session_project(session, config)
        .and_then(|p| config.repositories.iter().find(|r| r.key == p.repository))
}

fn selected_env_name<'a>(session: &Session, config: &'a Config) -> &'a str {
    session
        .environment_key
        .as_ref()
        .and_then(|k| config.environments.iter().find(|e| e.key == *k))
        .map(|e| e.name.as_str())
        .unwrap_or("?")
}

fn yesno(v: bool) -> &'static str {
    if v {
        "✅ Yes"
    } else {
        "❌ No"
    }
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn render_log_message(job: &Job, max_lines: usize) -> String {
    let mut text = format!(
        "📋 Log job #{}\nStatus: {}\nStage: {}\n\n",
        short_id(&job.job_id),
        job.status.label(),
        job.stage
    );

    text.push_str("Log mới nhất:\n");
    for line in job.log.iter().rev().take(max_lines.min(20)).rev() {
        text.push_str("- ");
        text.push_str(&compact_log_line(line));
        text.push('\n');
    }

    if let Some(error) = &job.error {
        text.push_str("\nLỗi:\n");
        text.push_str(&truncate_text(error, 1200));
    }

    text.push_str("\n\nLog đầy đủ nằm trong file log trên server.");
    truncate_text(&text, 3900)
}

fn compact_log_line(line: &str) -> String {
    let without_timestamp = line.split_once(' ').map(|(_, rest)| rest).unwrap_or(line);
    truncate_text(&without_timestamp.replace('\n', " "), 180)
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut output: String = value.chars().take(max_chars.saturating_sub(3)).collect();
    output.push_str("...");
    output
}

async fn edit_session_message(
    bot: &Bot,
    chat_id: ChatId,
    message_id: Option<MessageId>,
    text: &str,
    reply_markup: Option<InlineKeyboardMarkup>,
) {
    let msg_id = match message_id {
        Some(id) => id,
        None => return,
    };
    edit_md(bot, chat_id, msg_id, text, reply_markup).await;
}

async fn authenticate_or_reply(state: &AppState, bot: Bot, msg: &Message) -> Option<AuthContext> {
    let chat_id = msg.chat.id;
    let user_id = msg.from().map(|u| u.id.0 as i64)?;

    match auth::authenticate(&state.config, user_id, chat_id.0) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            send_plain(&bot, chat_id, &format!("❌ {}", e)).await.ok();
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_preset::{FastPreset, FastPresetAction};
    use crate::config::RepositoryConfig;
    use crate::menu;
    use std::path::PathBuf;

    fn make_repository(
        main_branch: &str,
        quick: Vec<&str>,
        patterns: Vec<&str>,
        forbidden: Vec<&str>,
    ) -> RepositoryConfig {
        RepositoryConfig {
            key: "test".to_string(),
            name: "Test".to_string(),
            repo_url: "git@github.com:test/test.git".to_string(),
            main_branch: main_branch.to_string(),
            quick_branches: quick.iter().map(|s| s.to_string()).collect(),
            manual_branch_enabled: true,
            manual_branch_patterns: patterns.iter().map(|s| s.to_string()).collect(),
            forbidden_branch_patterns: forbidden.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_preset(project: &str, environment: &str, branch: &str) -> FastPreset {
        FastPreset {
            id: "preset-1".to_string(),
            owner_user_id: 1,
            name: "WebPOS staging".to_string(),
            project: project.to_string(),
            environment: environment.to_string(),
            branch: branch.to_string(),
            action: FastPresetAction::Deploy,
        }
    }

    fn make_auth_context(can_deploy_staging: bool) -> AuthContext {
        AuthContext {
            user: crate::config::UserConfig {
                id: 1,
                name: "Test".to_string(),
                role: "tester".to_string(),
            },
            permissions: crate::config::RolePermissions {
                can_build: true,
                can_deploy_staging,
                can_deploy_production: false,
                can_rollback: false,
                can_view_logs: true,
                can_cancel_jobs: false,
            },
            chat_id: 100,
        }
    }

    fn make_preset_config(with_target: bool) -> crate::config::RawConfig {
        let deploy_targets = if with_target {
            vec![crate::config::DeployTargetConfig {
                project: "webpos".to_string(),
                environment: "staging".to_string(),
                iis_path: PathBuf::from("D:/wwwroot/WebPOS"),
                backup_root: PathBuf::from("D:/backups"),
                deploy_mode: "overlay".to_string(),
                use_app_offline: false,
                recycle_app_pool_after_deploy: false,
                app_pool_name: None,
                preserve_files: vec![],
                preserve_dirs: vec![],
            }]
        } else {
            vec![]
        };

        crate::config::RawConfig {
            app: crate::config::AppConfig {
                name: "Test".to_string(),
                timezone: "UTC".to_string(),
                data_dir: PathBuf::from("/tmp/data"),
                log_dir: PathBuf::from("/tmp/logs"),
                workspace_root: PathBuf::from("/tmp/workspace"),
            },
            telegram: crate::config::TelegramConfig {
                bot_token_env: "TEST_TOKEN".to_string(),
                allowed_chat_ids: vec![100],
            },
            users: vec![],
            roles: std::collections::HashMap::new(),
            tools: crate::config::ToolConfig {
                git_path: PathBuf::from("git"),
                msbuild_path: PathBuf::from("msbuild"),
                robocopy_path: PathBuf::from("robocopy"),
                seven_zip_path: PathBuf::from("7z"),
                appcmd_path: PathBuf::from("appcmd"),
            },
            defaults: crate::config::DefaultsConfig {
                build_timeout_minutes: 30,
                deploy_timeout_minutes: 15,
                backup_timeout_minutes: 30,
                max_log_lines_in_telegram: 80,
                project_lock_timeout_minutes: 60,
                keep_staging_on_failure: true,
                keep_success_staging: false,
            },
            environments: vec![crate::config::EnvironmentConfig {
                key: "staging".to_string(),
                name: "Staging".to_string(),
                requires_double_confirm: false,
            }],
            quick_deploy: None,
            repositories: vec![make_repository(
                "master",
                vec!["s3-retail-prod"],
                vec!["feature/*"],
                vec!["backup/*"],
            )],
            projects: vec![crate::config::ProjectConfig {
                key: "webpos".to_string(),
                name: "WebPOS".to_string(),
                repository: "test".to_string(),
                project_file: PathBuf::from("Websites/WebPOS/WebPOS.csproj"),
                configuration: "Release".to_string(),
                precompile_before_publish: true,
                enable_updateable: true,
                delete_from_build: vec![],
            }],
            deploy_targets,
        }
    }

    #[test]
    fn fast_preset_validation_rejects_missing_deploy_target() {
        let config = make_preset_config(false);
        let ctx = make_auth_context(true);
        let preset = make_preset("webpos", "staging", "s3-retail-prod");

        let result = validate_preset_for_user(&ctx, &config, &preset);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("deploy target"));
    }

    #[test]
    fn fast_preset_validation_rejects_invalid_branch() {
        let config = make_preset_config(true);
        let ctx = make_auth_context(true);
        let preset = make_preset("webpos", "staging", "backup/test");

        let result = validate_preset_for_user(&ctx, &config, &preset);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Branch"));
    }

    #[test]
    fn fast_preset_validation_rejects_missing_permission() {
        let config = make_preset_config(true);
        let ctx = make_auth_context(false);
        let preset = make_preset("webpos", "staging", "s3-retail-prod");

        let result = validate_preset_for_user(&ctx, &config, &preset);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Permission"));
    }

    // ---- Branch validation tests ----

    #[test]
    fn test_valid_branch_release() {
        let repo = make_repository(
            "master",
            vec!["master", "develop"],
            vec!["release/*", "feature/*"],
            vec!["backup/*"],
        );
        let result = validate_manual_branch("release/2026-07-01", Some(&repo));
        assert!(result.is_ok(), "Expected OK, got: {:?}", result.err());
        assert_eq!(result.unwrap(), "release/2026-07-01");
    }

    #[test]
    fn test_valid_branch_hotfix() {
        let repo = make_repository(
            "master",
            vec!["master", "develop"],
            vec!["hotfix/*", "feature/*"],
            vec!["backup/*"],
        );
        let result = validate_manual_branch("hotfix/payment-qr", Some(&repo));
        assert!(result.is_ok(), "Expected OK, got: {:?}", result.err());
        assert_eq!(result.unwrap(), "hotfix/payment-qr");
    }

    #[test]
    fn test_valid_branch_feature() {
        let repo = make_repository(
            "master",
            vec!["master", "develop"],
            vec!["feature/*", "release/*"],
            vec!["backup/*"],
        );
        let result = validate_manual_branch("feature/new-pos-ui", Some(&repo));
        assert!(result.is_ok(), "Expected OK, got: {:?}", result.err());
        assert_eq!(result.unwrap(), "feature/new-pos-ui");
    }

    #[test]
    fn test_invalid_branch_empty() {
        let repo = make_repository("master", vec!["master"], vec!["feature/*"], vec![]);
        let result = validate_manual_branch("", Some(&repo));
        assert!(result.is_err(), "Expected error for empty branch");
    }

    #[test]
    fn test_invalid_branch_space() {
        let repo = make_repository("master", vec!["master"], vec!["feature/*"], vec![]);
        let result = validate_manual_branch("my branch", Some(&repo));
        assert!(result.is_err(), "Expected error for space branch");
        let err = result.err().unwrap();
        assert!(
            err.contains("whitespace"),
            "Should mention whitespace: {}",
            err
        );
    }

    #[test]
    fn test_invalid_branch_path_traversal() {
        let repo = make_repository("master", vec!["master"], vec!["feature/*"], vec![]);
        let result = validate_manual_branch("../../x", Some(&repo));
        assert!(result.is_err(), "Expected error for path traversal");
    }

    #[test]
    fn test_invalid_branch_semicolon() {
        let repo = make_repository("master", vec!["master"], vec!["release/*"], vec![]);
        let result = validate_manual_branch("release/x;del", Some(&repo));
        assert!(result.is_err(), "Expected error for semicolon");
    }

    #[test]
    fn test_invalid_branch_starts_with_slash() {
        let repo = make_repository("master", vec!["master"], vec!["release/*"], vec![]);
        let result = validate_manual_branch("/abc", Some(&repo));
        assert!(result.is_err(), "Expected error for /abc");
    }

    #[test]
    fn test_invalid_branch_ends_with_slash() {
        let repo = make_repository("master", vec!["master"], vec!["release/*"], vec![]);
        let result = validate_manual_branch("abc/", Some(&repo));
        assert!(result.is_err(), "Expected error for abc/");
    }

    #[test]
    fn test_invalid_branch_double_slash() {
        let repo = make_repository("master", vec!["master"], vec!["release/*"], vec![]);
        let result = validate_manual_branch("abc//def", Some(&repo));
        assert!(result.is_err(), "Expected error for double slash");
    }

    #[test]
    fn test_invalid_branch_forbidden_pattern() {
        let repo = make_repository(
            "master",
            vec!["master"],
            vec!["feature/*"],
            vec!["backup/*"],
        );
        let result = validate_manual_branch("backup/test", Some(&repo));
        assert!(result.is_err(), "Expected error for forbidden pattern");
        let err = result.err().unwrap();
        assert!(
            err.contains("forbidden"),
            "Should mention forbidden: {}",
            err
        );
    }

    #[test]
    fn test_forbidden_char_ampersand() {
        let repo = make_repository("master", vec!["master"], vec!["release/*"], vec![]);
        let result = validate_manual_branch("release/1&2", Some(&repo));
        assert!(result.is_err(), "Expected error for &");
    }

    #[test]
    fn test_max_length() {
        let repo = make_repository("master", vec!["master"], vec!["feature/*"], vec![]);
        let long = "feature/".to_string() + &"a".repeat(120);
        let result = validate_manual_branch(&long, Some(&repo));
        assert!(result.is_err(), "Expected error for long branch");
    }

    #[test]
    fn test_backslash_rejected() {
        let repo = make_repository("master", vec!["master"], vec!["feature/*"], vec![]);
        let result = validate_manual_branch("feature\\test", Some(&repo));
        assert!(result.is_err(), "Expected error for backslash");
    }

    #[test]
    fn test_single_quote_rejected() {
        let repo = make_repository("master", vec!["master"], vec!["feature/*"], vec![]);
        let result = validate_manual_branch("feature/'test", Some(&repo));
        assert!(result.is_err(), "Expected error for single quote");
    }

    // ---- Permission tests ----

    #[test]
    fn test_build_only_permission_checks_build() {
        use crate::config::RolePermissions;
        let ctx = AuthContext {
            user: crate::config::UserConfig {
                id: 1,
                name: "Test".to_string(),
                role: "developer".to_string(),
            },
            permissions: RolePermissions {
                can_build: true,
                can_deploy_staging: false,
                can_deploy_production: false,
                can_rollback: false,
                can_view_logs: true,
                can_cancel_jobs: false,
            },
            chat_id: 100,
        };
        let config = crate::config::RawConfig {
            app: crate::config::AppConfig {
                name: "Test".to_string(),
                timezone: "UTC".to_string(),
                data_dir: PathBuf::from("/tmp/data"),
                log_dir: PathBuf::from("/tmp/logs"),
                workspace_root: PathBuf::from("/tmp/workspace"),
            },
            telegram: crate::config::TelegramConfig {
                bot_token_env: "TEST_TOKEN".to_string(),
                allowed_chat_ids: vec![100],
            },
            users: vec![],
            roles: std::collections::HashMap::new(),
            tools: crate::config::ToolConfig {
                git_path: PathBuf::from("git"),
                msbuild_path: PathBuf::from("msbuild"),
                robocopy_path: PathBuf::from("robocopy"),
                seven_zip_path: PathBuf::from("7z"),
                appcmd_path: PathBuf::from("appcmd"),
            },
            defaults: crate::config::DefaultsConfig {
                build_timeout_minutes: 30,
                deploy_timeout_minutes: 15,
                backup_timeout_minutes: 30,
                max_log_lines_in_telegram: 80,
                project_lock_timeout_minutes: 60,
                keep_staging_on_failure: true,
                keep_success_staging: false,
            },
            environments: vec![],
            quick_deploy: None,
            repositories: vec![],
            projects: vec![],
            deploy_targets: vec![],
        };
        // BuildOnly should check can_build
        let r1 = check_action_permission(&ctx, DeployAction::BuildOnly, "staging", &config);
        assert!(r1.is_ok(), "BuildOnly should pass with can_build=true");

        // Deploy to production should fail for developer without can_deploy_production
        let r2 =
            check_action_permission(&ctx, DeployAction::BackupAndDeploy, "production", &config);
        assert!(
            r2.is_err(),
            "Developer should not be able to deploy production"
        );

        // Deploy to staging should pass for developer with can_deploy_staging=false? wait we set it false
        let r3 = check_action_permission(&ctx, DeployAction::BackupAndDeploy, "staging", &config);
        assert!(
            r3.is_err(),
            "Developer with can_deploy_staging=false should fail"
        );

        // Test with proper permissions
        let ctx2 = AuthContext {
            user: crate::config::UserConfig {
                id: 2,
                name: "Dev".to_string(),
                role: "dev".to_string(),
            },
            permissions: RolePermissions {
                can_build: true,
                can_deploy_staging: true,
                can_deploy_production: false,
                can_rollback: false,
                can_view_logs: true,
                can_cancel_jobs: false,
            },
            chat_id: 100,
        };
        let r4 = check_action_permission(&ctx2, DeployAction::BackupAndDeploy, "staging", &config);
        assert!(
            r4.is_ok(),
            "Dev with can_deploy_staging=true should pass staging"
        );
    }

    // ---- Branch keyboard tests ----

    #[test]
    fn test_branch_keyboard_main_branch_comes_first() {
        // We can't easily assert position in InlineKeyboardMarkup via public API,
        // but we can at least verify that calling it doesn't panic and returns proper structure
        let quick = vec!["develop".to_string(), "master".to_string()];
        let kb = menu::branch_keyboard("master", &quick, false);
        let rows = kb.inline_keyboard;

        // First row should be main branch with star
        assert!(!rows.is_empty(), "Keyboard should have rows");
        let first_row = &rows[0];
        assert!(!first_row.is_empty(), "First row should have buttons");
        let first_btn_text = &first_row[0].text;
        assert!(
            first_btn_text.contains("⭐"),
            "First button should have star: {}",
            first_btn_text
        );
        assert!(
            first_btn_text.contains("master"),
            "First button should be master: {}",
            first_btn_text
        );
    }

    #[test]
    fn test_branch_keyboard_main_branch_not_in_quick() {
        // If main_branch is not in quick_branches, it should still appear first
        let quick = vec!["develop".to_string()];
        let kb = menu::branch_keyboard("main", &quick, false);
        let rows = kb.inline_keyboard;
        assert!(!rows.is_empty(), "Keyboard should have rows");
        let first_btn_text = &rows[0][0].text;
        assert!(
            first_btn_text.contains("⭐ main"),
            "First button should be ⭐ main: {}",
            first_btn_text
        );
    }

    #[test]
    fn test_branch_keyboard_develop_is_main_branch() {
        // If config has main_branch = "develop", then develop should be starred, not master
        let quick = vec!["master".to_string(), "develop".to_string()];
        let kb = menu::branch_keyboard("develop", &quick, false);
        let rows = kb.inline_keyboard;
        assert!(!rows.is_empty(), "Keyboard should have rows");
        let first_btn_text = &rows[0][0].text;
        assert!(
            first_btn_text.contains("⭐ develop"),
            "Develop should be main branch"
        );
        // Second row should be master without star
        if rows.len() > 1 {
            let second_btn_text = &rows[1][0].text;
            assert!(
                !second_btn_text.contains('⭐'),
                "Master should not have star"
            );
        }
    }

    #[test]
    fn test_escape_md_v2() {
        let input = "Hello _world_ *bold* [link] (parens) ~tilde~ `code` > quote #hash +plus -dash =equals |pipe {brace} .dot !excl";
        let escaped = escape_md_v2(input);
        assert_eq!(
            escaped,
            "Hello \\_world\\_ \\*bold\\* \\[link\\] \\(parens\\) \\~tilde\\~ \\`code\\` \\> quote \\#hash \\+plus \\-dash \\=equals \\|pipe \\{brace\\} \\.dot \\!excl"
        );
    }

    #[test]
    fn test_escape_md_v2_plain_text() {
        assert_eq!(escape_md_v2("abc123"), "abc123");
        assert_eq!(escape_md_v2(""), "");
    }

    #[test]
    fn test_validate_manual_branch_without_project() {
        // Validate without project (just basic checks)
        let result = validate_manual_branch("feature/test", None);
        assert!(result.is_ok(), "No project should still allow valid branch");
        assert_eq!(result.unwrap(), "feature/test");

        // Basic checks still apply
        let result = validate_manual_branch("", None);
        assert!(result.is_err(), "Empty should fail even without project");
    }
}
