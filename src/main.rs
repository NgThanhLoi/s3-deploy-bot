#![allow(dead_code)]

mod auth;
mod backup;
mod bot;
mod commands;
mod config;
mod deploy;
mod fast_preset;
mod git;
mod iis;
mod job;
mod log;
mod menu;
mod msbuild;
mod runner;
mod service;
mod session;
mod staging;
mod storage;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

use crate::config::Config;

#[derive(Parser)]
#[command(
    name = "s3-deploy-bot",
    version,
    about = "Telegram Deploy Bot for ASP.NET WebForms"
)]
struct Cli {
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,

    #[arg(long, help = "Run as a Windows Service. Use this from sc.exe binPath.")]
    service: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.service {
        service::run(cli.config)?;
        return Ok(());
    }

    let config = Config::from_file(&cli.config)?;
    let _guard = setup_tracing(&config)?;

    tracing::info!(
        "{} v{} started. Config loaded from: {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        cli.config.display()
    );

    tracing::info!(
        "Loaded {} user(s), {} project(s), {} environment(s), {} deploy target(s)",
        config.users.len(),
        config.projects.len(),
        config.environments.len(),
        config.deploy_targets.len()
    );

    let config = Arc::new(config);

    bot::run_bot(config).await?;

    tracing::info!("Bot stopped, goodbye.");
    Ok(())
}

fn setup_tracing(config: &Config) -> anyhow::Result<WorkerGuard> {
    let log_dir = &config.app.log_dir;
    std::fs::create_dir_all(log_dir)?;

    let file_appender = tracing_appender::rolling::daily(log_dir, "s3-deploy-bot.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_line_number(true)
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")));

    let console_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_target(true)
        .with_line_number(true)
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")));

    tracing_subscriber::registry()
        .with(file_layer)
        .with(console_layer)
        .init();

    tracing::info!("Logging to file: {}/s3-deploy-bot.log", log_dir.display());
    Ok(guard)
}
