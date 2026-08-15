//! Small, fail-loud wrappers around the Docker CLI.
//!
//! The Docker CLI is itself pinned and supplied by the service image. Every
//! call captures argv, exit status, stdout, and stderr. Ordinary control-plane
//! operations have a finite diagnostic timeout; `docker wait` deliberately
//! does not, because a valid agent session has no artificial wall-time limit.

use std::ffi::OsStr;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::error::{ServiceError, ServiceResult};

const DOCKER_OP_TIMEOUT: Duration = Duration::from_secs(120);

pub async fn run_docker<I, S>(args: I, op_label: &str) -> ServiceResult<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let argv = args
        .into_iter()
        .map(|value| value.as_ref().to_os_string())
        .collect::<Vec<_>>();
    let rendered = argv
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");

    let mut command = Command::new("docker");
    command
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let future = command.output();
    let output = tokio::time::timeout(DOCKER_OP_TIMEOUT, future)
        .await
        .map_err(|_| {
            ServiceError::Timeout(format!(
                "{op_label}: `docker {rendered}` exceeded {DOCKER_OP_TIMEOUT:?}"
            ))
        })?
        .map_err(|e| {
            ServiceError::DockerCommand(format!(
                "{op_label}: failed to spawn `docker {rendered}`: {e}"
            ))
        })?;

    if !output.status.success() {
        let code = output
            .status
            .code()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<signal>".into());
        return Err(ServiceError::DockerCommand(format!(
            "{op_label}: `docker {rendered}` exited {code}; stderr: {}; stdout: {}",
            truncate(&String::from_utf8_lossy(&output.stderr), 4096),
            truncate(&String::from_utf8_lossy(&output.stdout), 1024)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub async fn ping_daemon() -> ServiceResult<String> {
    run_docker(
        ["version", "--format", "{{.Server.Version}}"],
        "docker_version",
    )
    .await
    .map(|value| value.trim().to_string())
    .map_err(|error| match error {
        ServiceError::DockerCommand(message) => ServiceError::DockerUnavailable(format!(
            "Docker daemon is not reachable through the pinned socket: {message}"
        )),
        other => other,
    })
}

pub async fn image_id(tag: &str) -> ServiceResult<String> {
    run_docker(
        ["image", "inspect", "--format", "{{.Id}}", tag],
        &format!("image_id({tag})"),
    )
    .await
    .map(|value| value.trim().to_string())
    .map_err(|error| match error {
        ServiceError::DockerCommand(message) => ServiceError::ImageMissing(format!(
            "required image {tag:?} is absent or unreadable: {message}"
        )),
        other => other,
    })
}

pub async fn inspect_format(object: &str, format: &str, op: &str) -> ServiceResult<String> {
    run_docker(["inspect", "--format", format, object], op)
        .await
        .map(|value| value.trim_end_matches(['\r', '\n']).to_string())
}

pub async fn run_detached<I, S>(args: I, op_label: &str) -> ServiceResult<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut argv = vec![
        std::ffi::OsString::from("run"),
        std::ffi::OsString::from("-d"),
    ];
    argv.extend(args.into_iter().map(|value| value.as_ref().to_os_string()));
    let id = run_docker(argv, op_label).await?.trim().to_string();
    if id.len() != 64 || !id.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(ServiceError::DockerCommand(format!(
            "{op_label}: docker run returned an invalid container ID {id:?}"
        )));
    }
    Ok(id)
}

/// Wait indefinitely for the container. Cancellation is implemented by
/// stopping the container from the sibling select branch; Docker then makes
/// this wait return. Dropping the future kills the local CLI child.
pub async fn container_wait(name: &str) -> ServiceResult<i32> {
    let mut command = Command::new("docker");
    command
        .args(["wait", name])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = command.output().await.map_err(|e| {
        ServiceError::DockerCommand(format!("container_wait({name}): spawn/wait failed: {e}"))
    })?;
    if !output.status.success() {
        return Err(ServiceError::DockerCommand(format!(
            "container_wait({name}): docker wait failed; stderr: {}; stdout: {}",
            truncate(&String::from_utf8_lossy(&output.stderr), 4096),
            truncate(&String::from_utf8_lossy(&output.stdout), 1024)
        )));
    }
    output
        .stdout
        .split(|byte| byte.is_ascii_whitespace())
        .find(|part| !part.is_empty())
        .ok_or_else(|| ServiceError::DockerCommand(format!("container_wait({name}): empty output")))
        .and_then(|part| {
            std::str::from_utf8(part)
                .map_err(|e| {
                    ServiceError::DockerCommand(format!(
                        "container_wait({name}): non-UTF-8 exit code: {e}"
                    ))
                })?
                .parse::<i32>()
                .map_err(|e| {
                    ServiceError::DockerCommand(format!(
                        "container_wait({name}): invalid exit code: {e}"
                    ))
                })
        })
}

pub async fn verify_network_none(container: &str) -> ServiceResult<()> {
    let mode = inspect_format(
        container,
        "{{.HostConfig.NetworkMode}}",
        &format!("verify_network_none_mode({container})"),
    )
    .await?;
    if mode != "none" {
        return Err(ServiceError::Internal(format!(
            "agent {container} network mode is {mode:?}, expected exactly `none`"
        )));
    }
    let routes = run_docker(
        ["exec", container, "ip", "-4", "route", "show"],
        &format!("verify_network_none_routes({container})"),
    )
    .await?;
    if !routes.trim().is_empty() {
        return Err(ServiceError::Internal(format!(
            "agent {container} has IPv4 routes despite --network none: {:?}",
            routes.trim()
        )));
    }
    Ok(())
}

pub async fn verify_shared_network_namespace(proxy: &str, agent: &str) -> ServiceResult<()> {
    let agent_id =
        inspect_format(agent, "{{.Id}}", &format!("shared_net_agent_id({agent})")).await?;
    let mode = inspect_format(
        proxy,
        "{{.HostConfig.NetworkMode}}",
        &format!("shared_net_proxy_mode({proxy})"),
    )
    .await?;
    let expected = format!("container:{agent_id}");
    if mode != expected {
        return Err(ServiceError::Internal(format!(
            "inner proxy {proxy} network mode is {mode:?}, expected {expected:?}"
        )));
    }
    Ok(())
}

pub async fn container_running(name: &str) -> ServiceResult<bool> {
    inspect_format(
        name,
        "{{.State.Running}}",
        &format!("container_running({name})"),
    )
    .await
    .and_then(|value| match value.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(ServiceError::DockerCommand(format!(
            "container_running({name}): unexpected inspect value {other:?}"
        ))),
    })
}

pub async fn container_logs_tail(name: &str, tail: u32) -> ServiceResult<String> {
    run_docker(
        ["logs", "--tail", &tail.to_string(), name],
        &format!("container_logs_tail({name})"),
    )
    .await
}

pub async fn container_stop(name: &str, grace_secs: u32) -> ServiceResult<()> {
    run_docker(
        ["stop", "-t", &grace_secs.to_string(), name],
        &format!("container_stop({name})"),
    )
    .await
    .map(|_| ())
}

pub async fn container_force_remove(name: &str) -> ServiceResult<()> {
    run_docker(["rm", "-f", name], &format!("container_rm({name})"))
        .await
        .map(|_| ())
}

fn truncate(value: &str, max: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max {
        value.to_string()
    } else {
        format!(
            "{}…(truncated)",
            value.chars().take(max).collect::<String>()
        )
    }
}
