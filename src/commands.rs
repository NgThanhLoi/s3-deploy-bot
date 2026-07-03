use std::sync::Arc;

use teloxide::macros::BotCommands;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId, ParseMode};
use teloxide::utils::command::BotCommands as _;

use crate::auth::{self, AuthContext, Permission};
use crate::config::Config;
use crate::menu;
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
                "👋 Welcome, {}!\n\n\
                 Role: {}\n\
                 Chat ID: {}\n\n\
                 Available commands:\n{}",
                ctx.user.name,
                ctx.user.role,
                chat_id.0,
                Command::descriptions()
            ),
            Err(e) => format!(
                "❌ Access denied:\n{}\n\n\
                 Make sure your user ID ({}) and chat ID ({}) are in the config.",
                e, uid, chat_id.0
            ),
        },
        None => "❌ Could not identify you. Telegram user info is missing.".to_string(),
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
                "📋 Your Info\n\n\
                 User ID: {}\n\
                 Chat ID: {}\n\
                 Name: {}\n\
                 Role: {}\n\n\
                 Permissions:\n\
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
                "❌ Not authorized:\n{}\n\n\
                 User ID: {}\n\
                 Chat ID: {}",
                e, uid, chat_id.0
            ),
        },
        None => format!(
            "Anonymous user.\nChat ID: {}\n\n\
             Please start a private chat with the bot to identify yourself.",
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
            "⚠️ You already have an active session.\n\
             Use /cancel to end it, then /deploy again.",
        )
        .await?;
        return Ok(());
    }

    let session = state.session_store.create(ctx.user.id, chat_id.0).await;

    let text = "🚀 *Deploy Wizard*\n\nStep 1\\: Select an environment";
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

    send_plain(
        &bot,
        chat_id,
        "📊 Status\n\nNo active jobs. (Phase 7 will add job tracking here.)",
    )
    .await?;

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

    send_plain(
        &bot,
        chat_id,
        "📋 Log\n\nNo recent jobs. (Phase 7 will add log retrieval here.)",
    )
    .await?;

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
                .text("No active session. Use /deploy to start.")
                .await?;
            return Ok(());
        }
    };

    // Only the session owner can press buttons
    if session.owner_user_id != user_id {
        bot.answer_callback_query(&callback_id)
            .text("This session belongs to another user.")
            .await?;
        return Ok(());
    }

    // Handle navigation
    match data.as_str() {
        "nav:cancel" => {
            state.session_store.remove(&session.session_id).await;
            edit_session_message(
                &bot,
                chat_id,
                session.message_id,
                "❌ Deploy cancelled",
                None,
            )
            .await;
            bot.answer_callback_query(callback_id).await?;
            return Ok(());
        }
        "nav:back" => {
            let prev = previous_step(session.step);
            session.set_step(prev);
            state.session_store.update(session.clone()).await;
            bot.answer_callback_query(callback_id).await?;
            show_step(&session, &state, &bot, chat_id).await;
            return Ok(());
        }
        _ => {}
    }

    match session.step {
        SessionStep::SelectEnvironment => {
            handle_env_selected(&mut session, &state, &bot, chat_id, &callback_id, &data).await?
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
                .text("Session in an unexpected state. Use /cancel to reset.")
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

    let proj = session
        .project_key
        .as_ref()
        .and_then(|k| state.config.projects.iter().find(|p| p.key == *k));

    match validate_manual_branch(raw_text, proj) {
        Ok(branch) => {
            session.branch = Some(branch);
            session.set_step(SessionStep::SelectAction);
            state.session_store.update(session.clone()).await;

            show_step(&session, &state, &bot, chat_id).await;
        }
        Err(e) => {
            send_plain(&bot, chat_id, &format!("❌ Invalid branch: {}", e)).await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Branch validation
// ---------------------------------------------------------------------------

/// Validate a manual branch name according to project rules.
/// Returns the trimmed branch name on success, or an error message on failure.
pub fn validate_manual_branch(
    input: &str,
    project: Option<&crate::config::ProjectConfig>,
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

    if let Some(proj) = project {
        // Must match at least one manual_branch_patterns
        if !proj.manual_branch_patterns.is_empty() {
            let matched = proj
                .manual_branch_patterns
                .iter()
                .any(|pat| glob_match(pat, trimmed));
            if !matched {
                return Err(format!(
                    "Branch '{}' does not match any allowed pattern: {:?}",
                    trimmed, proj.manual_branch_patterns
                ));
            }
        }

        // Must not match forbidden_branch_patterns
        if !proj.forbidden_branch_patterns.is_empty() {
            let forbidden = proj
                .forbidden_branch_patterns
                .iter()
                .any(|pat| glob_match(pat, trimmed));
            if forbidden {
                return Err(format!(
                    "Branch '{}' matches a forbidden pattern: {:?}",
                    trimmed, proj.forbidden_branch_patterns
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
            .text("Unknown environment.")
            .await?;
        return Ok(());
    }

    session.environment_key = Some(env_key.to_string());
    session.set_step(SessionStep::SelectProject);
    state.session_store.update(session.clone()).await;

    bot.answer_callback_query(callback_id).await?;
    show_step(session, state, bot, chat_id).await;
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
                .text("No environment selected.")
                .await?;
            return Ok(());
        }
    };

    // Validate project exists
    let proj = state.config.projects.iter().find(|p| p.key == proj_key);
    if proj.is_none() {
        bot.answer_callback_query(callback_id)
            .text("Unknown project.")
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
            .text("No deploy target for this project/environment.")
            .await?;
        return Ok(());
    }

    session.project_key = Some(proj_key.to_string());
    session.set_step(SessionStep::SelectBranch);
    state.session_store.update(session.clone()).await;

    bot.answer_callback_query(callback_id).await?;
    show_step(session, state, bot, chat_id).await;
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

    match data {
        "branch:manual" => {
            let msg = match proj {
                Some(p) => {
                    let examples: Vec<&str> = p
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
                None => "✍️ Enter branch name.".to_string(),
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
            let valid = match proj {
                Some(p) => branch == p.main_branch || p.quick_branches.iter().any(|b| b == branch),
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
            show_step(session, state, bot, chat_id).await;
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
                    .text(&format!("Permission denied: {}", e))
                    .await?;
                return Ok(());
            }
        }
        Err(e) => {
            session.action = saved_action; // restore
            bot.answer_callback_query(callback_id)
                .text(&format!("Auth error: {}", e))
                .await?;
            return Ok(());
        }
    }

    session.set_step(SessionStep::Confirm);
    state.session_store.update(session.clone()).await;

    bot.answer_callback_query(callback_id).await?;
    show_step(session, state, bot, chat_id).await;
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
                session.set_step(SessionStep::Done);
                state.session_store.update(session.clone()).await;

                // Phase 8+: queue actual job
                // For now, just report completion
                bot.answer_callback_query(callback_id)
                    .text("Deploy queued!")
                    .await?;
                let report = build_complete_text(session, state);
                edit_session_message(bot, chat_id, session.message_id, &report, None).await;
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
            let text = "🚀 *Deploy Wizard*\n\nStep 1\\: Select an environment".to_string();
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
                "🚀 *Deploy Wizard* — *{}*\n\nStep 2\\: Select a project",
                escape_md_v2(env_name)
            );

            let env_key = session.environment_key.as_deref().unwrap_or("");
            let keyboard = menu::project_keyboard(&state.config, env_key);
            (text, keyboard)
        }
        SessionStep::SelectBranch => {
            let proj = session
                .project_key
                .as_ref()
                .and_then(|k| state.config.projects.iter().find(|p| p.key == *k));

            let proj_name = proj.map(|p| &p.name[..]).unwrap_or("?");

            let text = format!(
                "🚀 *Deploy Wizard* — *{}*\n\nStep 3\\: Select a branch",
                escape_md_v2(proj_name)
            );

            let (main_branch, quick, manual) = proj
                .map(|p| {
                    (
                        p.main_branch.as_str(),
                        p.quick_branches.clone(),
                        p.manual_branch_enabled,
                    )
                })
                .unwrap_or(("master", vec![], false));
            let keyboard = menu::branch_keyboard(main_branch, &quick, manual);
            (text, keyboard)
        }
        SessionStep::WaitingManualBranch => {
            let proj_name = session
                .project_key
                .as_ref()
                .and_then(|k| state.config.projects.iter().find(|p| p.key == *k))
                .map(|p| &p.name[..])
                .unwrap_or("?");

            let text = format!(
                "✍️ Type the branch name for *{}*\n\n\
                 Valid patterns: `feature/*`, `bugfix/*`, `hotfix/*`, `release/*`, `dev/*`\n\n\
                 Send the branch name as a text message\\.",
                escape_md_v2(proj_name)
            );

            (text, InlineKeyboardMarkup::default())
        }
        SessionStep::SelectAction => {
            let text = "🚀 *Deploy Wizard*\n\nStep 4\\: Select action".to_string();
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
        SessionStep::Done => {
            let text = "✅ *Deploy completed*\n\nCheck /status for details.".to_string();
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
            "🔴 *Confirm deploy to {}*\n\n\
             Bạn sắp deploy Production\\.\n\
             Bot sẽ backup IIS hiện tại rồi copy đè file từ staging vào IIS path\\.\n\n\
             *Project:* {}\n\
             *Branch:* `{}`\n\
             *Action:* {}\n\
             *Target IIS:* {}",
            escape_md_v2(env_name),
            escape_md_v2(proj_name),
            escape_md_v2(branch),
            escape_md_v2(&action_label),
            escape_md_v2(&target)
        )
    } else {
        format!(
            "⚠️ *Confirm deploy*\n\n\
             *Environment:* {}\n\
             *Project:* {}\n\
             *Branch:* `{}`\n\
             *Commit:* `{}`\n\
             *Action:* {}\n\
             *Target IIS:* {}\n\
             *Backup:* sẽ tạo trước khi copy\n\
             *App offline:* không dùng\n\
             *Deploy mode:* overlay, không /MIR",
            escape_md_v2(env_name),
            escape_md_v2(proj_name),
            escape_md_v2(branch),
            escape_md_v2(commit),
            escape_md_v2(&action_label),
            escape_md_v2(&target)
        )
    }
}

fn build_complete_text(session: &Session, state: &AppState) -> String {
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
        "✅ Deploy queued\n\n\
         *Project:* {}\n\
         *Environment:* {}\n\
         *Branch:* `{}`\n\
         *Action:* {}\n\n\
         Job execution will be added in Phase 8\\.",
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
        SessionStep::Done => SessionStep::Done,
    }
}

fn yesno(v: bool) -> &'static str {
    if v {
        "✅ Yes"
    } else {
        "❌ No"
    }
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
    use crate::config::ProjectConfig;
    use crate::menu;
    use std::path::PathBuf;

    fn make_project(
        main_branch: &str,
        quick: Vec<&str>,
        patterns: Vec<&str>,
        forbidden: Vec<&str>,
    ) -> ProjectConfig {
        ProjectConfig {
            key: "test".to_string(),
            name: "Test".to_string(),
            repo_url: "git@github.com:test/test.git".to_string(),
            workspace: PathBuf::from("/tmp/test"),
            project_file: PathBuf::from("/tmp/test/test.csproj"),
            configuration: "Release".to_string(),
            main_branch: main_branch.to_string(),
            quick_branches: quick.iter().map(|s| s.to_string()).collect(),
            manual_branch_enabled: true,
            manual_branch_patterns: patterns.iter().map(|s| s.to_string()).collect(),
            forbidden_branch_patterns: forbidden.iter().map(|s| s.to_string()).collect(),
            delete_from_staging: vec![],
            delete_app_global_resources: false,
        }
    }

    // ---- Branch validation tests ----

    #[test]
    fn test_valid_branch_release() {
        let proj = make_project(
            "master",
            vec!["master", "develop"],
            vec!["release/*", "feature/*"],
            vec!["backup/*"],
        );
        let result = validate_manual_branch("release/2026-07-01", Some(&proj));
        assert!(result.is_ok(), "Expected OK, got: {:?}", result.err());
        assert_eq!(result.unwrap(), "release/2026-07-01");
    }

    #[test]
    fn test_valid_branch_hotfix() {
        let proj = make_project(
            "master",
            vec!["master", "develop"],
            vec!["hotfix/*", "feature/*"],
            vec!["backup/*"],
        );
        let result = validate_manual_branch("hotfix/payment-qr", Some(&proj));
        assert!(result.is_ok(), "Expected OK, got: {:?}", result.err());
        assert_eq!(result.unwrap(), "hotfix/payment-qr");
    }

    #[test]
    fn test_valid_branch_feature() {
        let proj = make_project(
            "master",
            vec!["master", "develop"],
            vec!["feature/*", "release/*"],
            vec!["backup/*"],
        );
        let result = validate_manual_branch("feature/new-pos-ui", Some(&proj));
        assert!(result.is_ok(), "Expected OK, got: {:?}", result.err());
        assert_eq!(result.unwrap(), "feature/new-pos-ui");
    }

    #[test]
    fn test_invalid_branch_empty() {
        let proj = make_project("master", vec!["master"], vec!["feature/*"], vec![]);
        let result = validate_manual_branch("", Some(&proj));
        assert!(result.is_err(), "Expected error for empty branch");
    }

    #[test]
    fn test_invalid_branch_space() {
        let proj = make_project("master", vec!["master"], vec!["feature/*"], vec![]);
        let result = validate_manual_branch("my branch", Some(&proj));
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
        let proj = make_project("master", vec!["master"], vec!["feature/*"], vec![]);
        let result = validate_manual_branch("../../x", Some(&proj));
        assert!(result.is_err(), "Expected error for path traversal");
    }

    #[test]
    fn test_invalid_branch_semicolon() {
        let proj = make_project("master", vec!["master"], vec!["release/*"], vec![]);
        let result = validate_manual_branch("release/x;del", Some(&proj));
        assert!(result.is_err(), "Expected error for semicolon");
    }

    #[test]
    fn test_invalid_branch_starts_with_slash() {
        let proj = make_project("master", vec!["master"], vec!["release/*"], vec![]);
        let result = validate_manual_branch("/abc", Some(&proj));
        assert!(result.is_err(), "Expected error for /abc");
    }

    #[test]
    fn test_invalid_branch_ends_with_slash() {
        let proj = make_project("master", vec!["master"], vec!["release/*"], vec![]);
        let result = validate_manual_branch("abc/", Some(&proj));
        assert!(result.is_err(), "Expected error for abc/");
    }

    #[test]
    fn test_invalid_branch_double_slash() {
        let proj = make_project("master", vec!["master"], vec!["release/*"], vec![]);
        let result = validate_manual_branch("abc//def", Some(&proj));
        assert!(result.is_err(), "Expected error for double slash");
    }

    #[test]
    fn test_invalid_branch_forbidden_pattern() {
        let proj = make_project(
            "master",
            vec!["master"],
            vec!["feature/*"],
            vec!["backup/*"],
        );
        let result = validate_manual_branch("backup/test", Some(&proj));
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
        let proj = make_project("master", vec!["master"], vec!["release/*"], vec![]);
        let result = validate_manual_branch("release/1&2", Some(&proj));
        assert!(result.is_err(), "Expected error for &");
    }

    #[test]
    fn test_max_length() {
        let proj = make_project("master", vec!["master"], vec!["feature/*"], vec![]);
        let long = "feature/".to_string() + &"a".repeat(120);
        let result = validate_manual_branch(&long, Some(&proj));
        assert!(result.is_err(), "Expected error for long branch");
    }

    #[test]
    fn test_backslash_rejected() {
        let proj = make_project("master", vec!["master"], vec!["feature/*"], vec![]);
        let result = validate_manual_branch("feature\\test", Some(&proj));
        assert!(result.is_err(), "Expected error for backslash");
    }

    #[test]
    fn test_single_quote_rejected() {
        let proj = make_project("master", vec!["master"], vec!["feature/*"], vec![]);
        let result = validate_manual_branch("feature/'test", Some(&proj));
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
