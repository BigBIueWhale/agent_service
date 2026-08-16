//! Bootstrap. Loads config, performs preflight, binds the listener (loopback
//! only), serves until SIGINT/SIGTERM, then drives a graceful shutdown that
//! cancels any in-flight session and waits for its teardown to complete
//! before exiting. Every failure path returns a non-zero exit code with a
//! single line describing the failure.

mod api;
mod bundle;
mod config;
mod docker_ops;
mod error;
mod result_parse;
mod runtime;
mod session;
mod staging;
mod validation;

use std::sync::Arc;

use crate::api::{pre_flight, AppState};
use crate::config::Config;
use crate::error::ServiceError;
use crate::runtime::Manager;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> std::process::ExitCode {
    if let Err(error) = init_tracing() {
        eprintln!("agent_service: tracing initialization failed: {error}");
        return std::process::ExitCode::from(2);
    }

    let cfg = match Config::load() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("agent_service: config error: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    tracing::info!(
        listen = %cfg.listen_addr,
        profile = %cfg.lock.profile,
        vllm = %cfg.vllm_endpoint,
        model = %cfg.vllm_model_name,
        agent_image = %cfg.agent_image,
        state_dir = %cfg.state_dir.display(),
        results_dir = %cfg.results_dir.display(),
        qwen_code_version = config::QWEN_CODE_VERSION,
        "agent_service starting"
    );

    if let Err(e) = pre_flight(&cfg).await {
        eprintln!("agent_service: pre-flight failed: {e}");
        return preflight_exit_code(&e);
    }

    let manager = Arc::new(Manager::new(Arc::clone(&cfg)));
    let state = AppState {
        cfg: Arc::clone(&cfg),
        manager: Arc::clone(&manager),
    };

    let listener = match tokio::net::TcpListener::bind(cfg.listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("agent_service: cannot bind {}: {e}", cfg.listen_addr);
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
    let shutdown_signals = match install_shutdown_signals() {
        Ok(signals) => signals,
        Err(error) => {
            eprintln!("agent_service: cannot install exact shutdown handlers: {error}");
            return std::process::ExitCode::from(1);
        }
    };
    tracing::info!(addr = %actual_addr, "listening (loopback only)");
    println!(
        "SERVICE_READY profile={} listen={} network=none",
        cfg.lock.profile, actual_addr
    );

    let app = api::router(state);

    let shutdown = tokio_util::sync::CancellationToken::new();
    let graceful_shutdown = shutdown.clone().cancelled_owned();
    let serve_future = async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(graceful_shutdown)
            .await
    };
    tokio::pin!(serve_future);

    // A signal broadcasts shutdown to both layers at the same time. This is
    // essential for a live GET /wait request: cancelling the detached agent
    // task lets that request reach its terminal body, while Axum keeps the
    // existing connection alive long enough to drain it. Waiting for HTTP to
    // drain before cancelling the agent would create a circular wait.
    let serve_result;
    let session_shutdown_outcome;
    tokio::select! {
        _ = wait_for_signal(shutdown_signals) => {
            shutdown.cancel();
            tracing::info!("shutdown broadcast; draining HTTP while cancelling the active session");
            (serve_result, session_shutdown_outcome) =
                tokio::join!(&mut serve_future, manager.shutdown());
        }
        result = &mut serve_future => {
            serve_result = result;
            shutdown.cancel();
            tracing::error!("HTTP server ended before a shutdown signal; cancelling the active session");
            session_shutdown_outcome = manager.shutdown().await;
        }
    }

    // Surface BOTH outcomes — a serve error AND a shutdown overrun are both
    // worth knowing about, and they often correlate.
    match (serve_result, session_shutdown_outcome) {
        (Ok(()), Ok(())) => {
            tracing::info!("shutdown complete");
            std::process::ExitCode::SUCCESS
        }
        (Ok(()), Err(e)) => {
            eprintln!("agent_service: session-level shutdown failed: {e}");
            std::process::ExitCode::from(3)
        }
        (Err(e), Ok(())) => {
            eprintln!("agent_service: server error: {e}");
            std::process::ExitCode::from(1)
        }
        (Err(server_err), Err(shutdown_err)) => {
            eprintln!(
                "agent_service: server error: {server_err}; \
                 session-level shutdown also failed: {shutdown_err}"
            );
            std::process::ExitCode::from(1)
        }
    }
}

struct ShutdownSignals {
    sigint: tokio::signal::unix::Signal,
    sigterm: tokio::signal::unix::Signal,
}

fn install_shutdown_signals() -> Result<ShutdownSignals, String> {
    let sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|error| format!("install SIGINT handler: {error}"))?;
    let sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| format!("install SIGTERM handler: {error}"))?;
    Ok(ShutdownSignals { sigint, sigterm })
}

/// Both streams are installed before `SERVICE_READY` is emitted. A closed
/// stream initiates the same graceful path and is logged as a kernel/runtime
/// fault; it never activates a one-signal or wait-forever fallback.
async fn wait_for_signal(mut signals: ShutdownSignals) {
    tokio::select! {
        sig = signals.sigint.recv() => {
            match sig {
                Some(()) => {
                    tracing::info!("received SIGINT; initiating graceful shutdown");
                }
                None => {
                    tracing::error!(
                        "SIGINT signal stream closed unexpectedly; initiating graceful shutdown anyway"
                    );
                }
            }
        }
        sig = signals.sigterm.recv() => {
            match sig {
                Some(()) => {
                    tracing::info!("received SIGTERM; initiating graceful shutdown");
                }
                None => {
                    tracing::error!(
                        "SIGTERM signal stream closed unexpectedly; initiating graceful shutdown anyway"
                    );
                }
            }
        }
    }
}

fn init_tracing() -> Result<(), String> {
    // Logging is part of the one pinned mode: RUST_LOG is intentionally not
    // consulted, so a host environment cannot silently change diagnostics.
    let filter = tracing_subscriber::EnvFilter::try_new("info,tower_http=warn,axum=warn")
        .map_err(|error| format!("the compiled logging filter is invalid: {error}"))?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .compact()
        .try_init()
        .map_err(|error| format!("cannot install the tracing subscriber: {error}"))
}

fn preflight_exit_code(e: &ServiceError) -> std::process::ExitCode {
    match e {
        ServiceError::DockerUnavailable(_) => std::process::ExitCode::from(10),
        ServiceError::Internal(_) => std::process::ExitCode::from(12),
        _ => std::process::ExitCode::from(1),
    }
}
