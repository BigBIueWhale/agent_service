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
mod network;
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
        agent_memory = %cfg.agent_memory_limit,
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
    tracing::info!(addr = %actual_addr, "listening (loopback only)");

    let app = api::router(state);

    let signal_future = wait_for_signal();

    // axum's `with_graceful_shutdown` stops accepting new connections and
    // drains in-flight HTTP requests when the future resolves. We do NOT
    // run `manager.shutdown()` inside that future — axum's drain only
    // covers HTTP-level work, and our session run-task is detached
    // (it lives past the HTTP request that submitted it). We drive the
    // session-level shutdown ourselves below, after axum returns.
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(signal_future)
        .await;

    // At this point: no more new HTTP requests will be accepted; in-flight
    // HTTP requests have drained. Detached run tasks may still be running.
    tracing::info!("HTTP server drained; awaiting session-level shutdown");

    let session_shutdown_outcome = manager.shutdown().await;

    // Surface BOTH outcomes — a serve error AND a shutdown overrun are both
    // worth knowing about, and they often correlate.
    match (serve_result, session_shutdown_outcome) {
        (Ok(()), Ok(())) => {
            tracing::info!("shutdown complete");
            std::process::ExitCode::SUCCESS
        }
        (Ok(()), Err(e)) => {
            eprintln!("agent_service: session-level shutdown overran: {e}");
            std::process::ExitCode::from(3)
        }
        (Err(e), Ok(())) => {
            eprintln!("agent_service: server error: {e}");
            std::process::ExitCode::from(1)
        }
        (Err(server_err), Err(shutdown_err)) => {
            eprintln!(
                "agent_service: server error: {server_err}; \
                 session-level shutdown also overran: {shutdown_err}"
            );
            std::process::ExitCode::from(1)
        }
    }
}

/// Resolves on the first SIGINT or SIGTERM. If installing either handler
/// fails, log and resolve immediately so the server doesn't get stuck
/// running forever — operator can ctrl-C twice or `kill -9` if needed,
/// and a logged error makes the install failure visible.
async fn wait_for_signal() {
    let mut sigint = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                error = %e,
                "cannot install SIGINT handler — graceful shutdown via Ctrl-C is unavailable; \
                 use SIGTERM (default for `kill <pid>`) or SIGKILL if needed"
            );
            // Fall back to waiting on SIGTERM only.
            return wait_for_sigterm_only().await;
        }
    };
    let mut sigterm = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                error = %e,
                "cannot install SIGTERM handler — graceful shutdown via `kill <pid>` is unavailable; \
                 SIGINT (Ctrl-C) is still wired"
            );
            // Fall back to SIGINT only.
            match sigint.recv().await {
                Some(()) => {
                    tracing::info!("received SIGINT; initiating graceful shutdown");
                }
                None => {
                    tracing::error!(
                        "SIGINT signal stream closed unexpectedly (kernel deregistered handler?); \
                         falling through to graceful shutdown without a triggering signal"
                    );
                }
            }
            return;
        }
    };
    tokio::select! {
        sig = sigint.recv() => {
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
        sig = sigterm.recv() => {
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

async fn wait_for_sigterm_only() {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut s) => match s.recv().await {
            Some(()) => {
                tracing::info!("received SIGTERM; initiating graceful shutdown");
            }
            None => {
                tracing::error!(
                    "SIGTERM signal stream closed unexpectedly; initiating graceful shutdown anyway"
                );
            }
        },
        Err(e) => {
            tracing::error!(
                error = %e,
                "cannot install SIGTERM handler either — graceful shutdown unavailable; \
                 the server will run until killed"
            );
            // Wait forever — the operator is on their own to kill the process.
            std::future::pending::<()>().await;
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
        ServiceError::ImageMissing(_) => std::process::ExitCode::from(11),
        ServiceError::Internal(_) => std::process::ExitCode::from(12),
        _ => std::process::ExitCode::from(1),
    }
}
