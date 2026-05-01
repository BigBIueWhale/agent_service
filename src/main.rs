//! Bootstrap. Loads config, performs preflight, binds the listener (loopback
//! only), and serves until SIGINT/SIGTERM. Every failure path returns a
//! non-zero exit code with a single line describing the failure.

mod api;
mod bundle;
mod config;
mod docker_ops;
mod error;
mod network;
mod result_parse;
mod session;
mod staging;
mod validation;

use std::sync::Arc;

use crate::api::{AppState, pre_flight};
use crate::config::Config;
use crate::error::ServiceError;
use crate::session::SessionManager;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> std::process::ExitCode {
    init_tracing();

    let cfg = match Config::load() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("agent_service: config error: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    tracing::info!(
        listen = %cfg.listen_addr,
        vllm = %format!("{}:{}", cfg.vllm_host, cfg.vllm_port),
        model = %cfg.vllm_model_name,
        agent_image = %cfg.agent_image,
        agent_memory = %cfg.agent_memory_limit,
        agent_storage_quota = ?cfg.agent_storage_quota,
        state_dir = %cfg.state_dir.display(),
        results_dir = %cfg.results_dir.display(),
        results_retain = cfg.results_retain,
        run_timeout_secs = cfg.run_timeout_secs,
        max_session_turns = cfg.max_session_turns,
        qwen_code_version = config::QWEN_CODE_VERSION,
        "agent_service starting"
    );

    if let Err(e) = pre_flight(&cfg).await {
        eprintln!("agent_service: pre-flight failed: {e}");
        return preflight_exit_code(&e);
    }

    let manager = Arc::new(SessionManager::new(Arc::clone(&cfg)));
    let state = AppState {
        cfg: Arc::clone(&cfg),
        manager,
    };

    let listener = match tokio::net::TcpListener::bind(cfg.listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "agent_service: cannot bind {}: {e}",
                cfg.listen_addr
            );
            return std::process::ExitCode::from(1);
        }
    };

    let actual_addr = match listener.local_addr() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("agent_service: local_addr() failed: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    if !actual_addr.ip().is_loopback() {
        eprintln!(
            "agent_service: refused to bind {actual_addr}: kernel returned a non-loopback address"
        );
        return std::process::ExitCode::from(1);
    }
    tracing::info!(addr = %actual_addr, "listening (loopback only)");

    let app = api::router(state);

    let shutdown = async {
        let mut sigint =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "cannot install SIGINT handler");
                    return;
                }
            };
        let mut sigterm =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "cannot install SIGTERM handler");
                    return;
                }
            };
        tokio::select! {
            _ = sigint.recv() => {
                tracing::info!("received SIGINT, shutting down");
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM, shutting down");
            }
        }
    };

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        eprintln!("agent_service: server error: {e}");
        return std::process::ExitCode::from(1);
    }

    tracing::info!("shutdown complete");
    std::process::ExitCode::SUCCESS
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new("info,tower_http=warn,axum=warn"))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .compact()
        .init();
}

fn preflight_exit_code(e: &ServiceError) -> std::process::ExitCode {
    match e {
        ServiceError::DockerUnavailable(_) => std::process::ExitCode::from(10),
        ServiceError::ImageMissing(_) => std::process::ExitCode::from(11),
        ServiceError::Internal(_) => std::process::ExitCode::from(12),
        _ => std::process::ExitCode::from(1),
    }
}
