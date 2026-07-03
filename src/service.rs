#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::time::Duration;

    use anyhow::{anyhow, Context, Result};
    use tokio::runtime::Runtime;
    use windows_service::define_windows_service;
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_dispatcher;

    use crate::config::Config;

    const SERVICE_NAME: &str = "S3DeployBot";
    static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    pub fn run(config_path: PathBuf) -> Result<()> {
        CONFIG_PATH
            .set(config_path)
            .map_err(|_| anyhow!("Windows service config path was already initialized"))?;
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .context("Failed to start Windows service dispatcher")
    }

    fn service_main(_arguments: Vec<OsString>) {
        if let Err(e) = run_service() {
            eprintln!("Windows service failed: {e:?}");
        }
    }

    fn run_service() -> Result<()> {
        let status_handle =
            service_control_handler::register(SERVICE_NAME, move |event| match event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    std::process::exit(0);
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            })
            .context("Failed to register Windows service control handler")?;

        status_handle
            .set_service_status(service_status(ServiceState::StartPending))
            .context("Failed to report service start pending")?;

        let config_path = CONFIG_PATH
            .get()
            .cloned()
            .ok_or_else(|| anyhow!("Missing Windows service config path"))?;
        let config = Config::from_file(&config_path)?;
        let _guard = crate::setup_tracing(&config)?;

        tracing::info!(
            "{} v{} service starting. Config: {}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            config_path.display()
        );

        status_handle
            .set_service_status(service_status(ServiceState::Running))
            .context("Failed to report service running")?;

        let runtime = Runtime::new().context("Failed to create Tokio runtime")?;
        runtime.block_on(crate::bot::run_bot(std::sync::Arc::new(config)))?;

        status_handle
            .set_service_status(service_status(ServiceState::Stopped))
            .context("Failed to report service stopped")?;
        Ok(())
    }

    fn service_status(current_state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state,
            controls_accepted: if current_state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;

    use anyhow::{bail, Result};

    pub fn run(_config_path: PathBuf) -> Result<()> {
        bail!("Windows service mode is only available on Windows builds")
    }
}

pub use imp::run;
