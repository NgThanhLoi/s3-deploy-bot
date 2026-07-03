use std::sync::Arc;

use teloxide::dispatching::UpdateFilterExt;
use teloxide::prelude::*;

use crate::commands::{self, AppState, Command};
use crate::config::Config;
use crate::session::SessionStore;

pub async fn run_bot(config: Arc<Config>) -> anyhow::Result<()> {
    let token = crate::config::get_bot_token(&config)?;
    let bot = Bot::new(token);

    let state = AppState {
        config,
        session_store: SessionStore::new(),
    };

    let command_handler = Update::filter_message()
        .filter_command::<Command>()
        .endpoint(handle_command);

    let callback_handler = Update::filter_callback_query().endpoint(handle_callback);

    let text_handler = Update::filter_message()
        .filter(|msg: Message| msg.text().is_some())
        .endpoint(handle_text);

    let handler = dptree::entry()
        .branch(command_handler)
        .branch(callback_handler)
        .branch(text_handler);

    tracing::info!("Starting Telegram bot...");
    tracing::info!(
        "Allowed chat IDs: {:?}",
        state.config.telegram.allowed_chat_ids
    );

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .default_handler(|upd| async move {
            tracing::warn!("Unhandled update: {:?}", upd);
        })
        .error_handler(LoggingErrorHandler::with_custom_text("Bot handler error"))
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn handle_command(
    msg: Message,
    bot: Bot,
    state: AppState,
    cmd: Command,
) -> Result<(), anyhow::Error> {
    match cmd {
        Command::Start => commands::handle_start(msg, bot, state).await,
        Command::Whoami => commands::handle_whoami(msg, bot, state).await,
        Command::Deploy => commands::handle_deploy(msg, bot, state).await,
        Command::Status => commands::handle_status(msg, bot, state).await,
        Command::Log => commands::handle_log(msg, bot, state).await,
        Command::Cancel => commands::handle_cancel(msg, bot, state).await,
    }
}

async fn handle_callback(q: CallbackQuery, bot: Bot, state: AppState) -> Result<(), anyhow::Error> {
    commands::handle_callback(q, bot, state).await
}

async fn handle_text(msg: Message, bot: Bot, state: AppState) -> Result<(), anyhow::Error> {
    commands::handle_text_message(msg, bot, state).await
}
