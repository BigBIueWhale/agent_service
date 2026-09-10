//! Fixed per-session Qwen stream capture.
//!
//! This tiny component is the sole per-session holder of the writable
//! `/output` mount. Qwen receives only a read-only `/streams` mount and can
//! connect once to each fixed Unix socket; it never sees `/output` itself.

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::ExitCode;

use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{Signal, SignalKind};

const EVENTS_SOCKET: &str = "/streams/events.sock";
const STDERR_SOCKET: &str = "/streams/stderr.sock";
const EVENTS_FILE: &str = "/output/events.jsonl";
const STDERR_FILE: &str = "/output/qwen.stderr";
const CAPTURE_ID: &str = "unix-stream-capture-v1";

#[cfg(test)]
#[path = "../../docker/tests/capture_owner.rs"]
mod capture_owner;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("session_capture: {error}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), String> {
    if std::env::args_os().count() != 1 {
        return Err("session_capture accepts no arguments".into());
    }
    run_body().await
}

async fn run_body() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 1000 || unsafe { libc::getegid() } != 1000 {
        return Err(format!(
            "capture identity drift: expected 1000:1000, observed {}:{}",
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() }
        ));
    }
    validate_directory(Path::new("/streams"), 0o700, "stream root")?;
    validate_directory(Path::new("/output"), 0o700, "output root")?;
    let mut termination = TerminationSignals::install()?;

    let events_file = create_private_output(Path::new(EVENTS_FILE))?;
    let stderr_file = create_private_output(Path::new(STDERR_FILE))?;
    let events_listener = bind_private_socket(Path::new(EVENTS_SOCKET))?;
    let stderr_listener = bind_private_socket(Path::new(STDERR_SOCKET))?;
    sync_directory(Path::new("/streams"), "sync capture socket publication")?;
    sync_directory(Path::new("/output"), "sync capture output publication")?;

    println!(
        "CAPTURE_READY capture={} events={} stderr={}",
        CAPTURE_ID, EVENTS_SOCKET, STDERR_SOCKET
    );
    #[cfg(test)]
    capture_owner::ready()?;

    let accepts = async {
        tokio::try_join!(
            accept_exact(events_listener, "events"),
            accept_exact(stderr_listener, "stderr")
        )
    };
    tokio::pin!(accepts);
    let accepted = tokio::select! {
        biased;
        result = &mut accepts => Some(result?),
        signal = termination.recv() => {
            signal?;
            None
        },
    };
    drop(accepts);
    let Some((events_stream, stderr_stream)) = accepted else {
        remove_owned_socket(Path::new(EVENTS_SOCKET))?;
        remove_owned_socket(Path::new(STDERR_SOCKET))?;
        sync_directory(Path::new("/streams"), "sync aborted capture sockets")?;
        sync_std_outputs(&events_file, &stderr_file)?;
        println!("CAPTURE_ABORTED capture={CAPTURE_ID} phase=accept");
        return Ok(());
    };
    remove_owned_socket(Path::new(EVENTS_SOCKET))?;
    remove_owned_socket(Path::new(STDERR_SOCKET))?;
    sync_directory(Path::new("/streams"), "sync accepted capture sockets")?;

    let mut events_stream = events_stream;
    let mut stderr_stream = stderr_stream;
    let mut events_file = tokio::fs::File::from_std(events_file);
    let mut stderr_file = tokio::fs::File::from_std(stderr_file);
    let copy_result = {
        let copies = async {
            tokio::try_join!(
                copy_stream(&mut events_stream, &mut events_file, "events"),
                copy_stream(&mut stderr_stream, &mut stderr_file, "stderr")
            )
        };
        tokio::pin!(copies);
        tokio::select! {
            biased;
            result = &mut copies => Some(result),
            signal = termination.recv() => match signal {
                Ok(()) => None,
                Err(error) => Some(Err(error)),
            },
        }
    };
    let sync_result = sync_async_outputs(&mut events_file, &mut stderr_file).await;
    let (events_bytes, stderr_bytes) = match (copy_result, sync_result) {
        (Some(Err(copy_error)), Err(sync_error)) => {
            return Err(format!("{copy_error}; {sync_error}"));
        }
        (Some(Err(error)), Ok(())) | (_, Err(error)) => return Err(error),
        (None, Ok(())) => {
            println!("CAPTURE_ABORTED capture={CAPTURE_ID} phase=copy");
            return Ok(());
        }
        (Some(Ok(counts)), Ok(())) => counts,
    };
    #[cfg(test)]
    capture_owner::before_complete(events_bytes, stderr_bytes)?;
    println!(
        "CAPTURE_COMPLETE capture={} events_bytes={} stderr_bytes={}",
        CAPTURE_ID, events_bytes, stderr_bytes
    );

    termination.recv().await?;
    println!("CAPTURE_STOPPED capture={CAPTURE_ID}");
    Ok(())
}

fn validate_directory(path: &Path, mode: u32, label: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("stat {label} {}: {error}", path.display()))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.permissions().mode() & 0o777 != mode
    {
        return Err(format!(
            "{label} drift at {}: type={:?} uid={} gid={} mode={:o}",
            path.display(),
            metadata.file_type(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777
        ));
    }
    Ok(())
}

fn create_private_output(path: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            format!(
                "create exclusive capture output {}: {error}",
                path.display()
            )
        })
}

fn bind_private_socket(path: &Path) -> Result<UnixListener, String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(metadata) => {
            return Err(format!(
                "refusing pre-existing capture socket path {} of type {:?}",
                path.display(),
                metadata.file_type()
            ));
        }
        Err(error) => return Err(format!("stat capture socket {}: {error}", path.display())),
    }
    let listener = std::os::unix::net::UnixListener::bind(path)
        .map_err(|error| format!("bind capture socket {}: {error}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("chmod capture socket {}: {error}", path.display()))?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("stat bound capture socket {}: {error}", path.display()))?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(format!(
            "bound capture socket drift at {}: socket={} uid={} gid={} mode={:o}",
            path.display(),
            metadata.file_type().is_socket(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777
        ));
    }
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("set capture socket nonblocking {}: {error}", path.display()))?;
    UnixListener::from_std(listener)
        .map_err(|error| format!("adopt capture socket {}: {error}", path.display()))
}

async fn accept_exact(listener: UnixListener, label: &str) -> Result<UnixStream, String> {
    let (stream, address) = listener
        .accept()
        .await
        .map_err(|error| format!("accept sole {label} stream: {error}"))?;
    if address.as_pathname().is_some() {
        return Err(format!(
            "{label} capture peer unexpectedly used a pathname: {address:?}"
        ));
    }
    Ok(stream)
}

async fn copy_stream(
    stream: &mut UnixStream,
    file: &mut tokio::fs::File,
    label: &str,
) -> Result<u64, String> {
    let result = tokio::io::copy(stream, file).await;
    #[cfg(test)]
    capture_owner::after_copy(label, &result)?;
    result.map_err(|error| format!("capture {label} stream: {error}"))
}

async fn flush_output(file: &mut tokio::fs::File, label: &str) -> Result<(), String> {
    #[cfg(test)]
    capture_owner::before_io("flush", label)
        .await
        .map_err(|error| format!("flush captured {label}: {error}"))?;
    let result = file.flush().await;
    #[cfg(test)]
    capture_owner::after_io("flush", label, &result)
        .map_err(|error| format!("observe flush captured {label}: {error}"))?;
    result.map_err(|error| format!("flush captured {label}: {error}"))
}

async fn sync_output(file: &tokio::fs::File, label: &str) -> Result<(), String> {
    #[cfg(test)]
    capture_owner::before_io("sync", label)
        .await
        .map_err(|error| format!("sync captured {label}: {error}"))?;
    let result = file.sync_all().await;
    #[cfg(test)]
    capture_owner::after_io("sync", label, &result)
        .map_err(|error| format!("observe sync captured {label}: {error}"))?;
    result.map_err(|error| format!("sync captured {label}: {error}"))
}

async fn sync_async_outputs(
    events_file: &mut tokio::fs::File,
    stderr_file: &mut tokio::fs::File,
) -> Result<(), String> {
    // Both files can own blocking writes even after a copy future is dropped.
    // Finalize each one, and retain every observed failure before returning.
    let results = [
        flush_output(events_file, "events").await,
        flush_output(stderr_file, "stderr").await,
        sync_output(events_file, "events").await,
        sync_output(stderr_file, "stderr").await,
    ];
    capture_io_result(results.into_iter().filter_map(Result::err).collect())
}

fn sync_std_outputs(
    events_file: &std::fs::File,
    stderr_file: &std::fs::File,
) -> Result<(), String> {
    let mut failures = Vec::new();
    retain_io_failure(
        events_file.sync_all(),
        "sync aborted captured events",
        &mut failures,
    );
    retain_io_failure(
        stderr_file.sync_all(),
        "sync aborted captured stderr",
        &mut failures,
    );
    capture_io_result(failures)
}

fn retain_io_failure(result: io::Result<()>, context: &str, failures: &mut Vec<String>) {
    if let Err(error) = result {
        failures.push(format!("{context}: {error}"));
    }
}

fn capture_io_result(failures: Vec<String>) -> Result<(), String> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn remove_owned_socket(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("stat accepted socket {}: {error}", path.display()))?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(format!(
            "refusing to remove drifted capture socket {}",
            path.display()
        ));
    }
    std::fs::remove_file(path)
        .map_err(|error| format!("remove accepted capture socket {}: {error}", path.display()))
}

fn sync_directory(path: &Path, label: &str) -> Result<(), String> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("{label} {}: {error}", path.display()))
}

struct TerminationSignals {
    terminate: Signal,
    interrupt: Signal,
}

impl TerminationSignals {
    fn install() -> Result<Self, String> {
        Ok(Self {
            terminate: tokio::signal::unix::signal(SignalKind::terminate())
                .map_err(|error| format!("install capture SIGTERM handler: {error}"))?,
            interrupt: tokio::signal::unix::signal(SignalKind::interrupt())
                .map_err(|error| format!("install capture SIGINT handler: {error}"))?,
        })
    }

    async fn recv(&mut self) -> Result<(), String> {
        tokio::select! {
            value = self.terminate.recv() => value.ok_or_else(|| "capture SIGTERM stream closed".to_string()),
            value = self.interrupt.recv() => value.ok_or_else(|| "capture SIGINT stream closed".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_paths_and_identity_are_fixed() {
        assert_eq!(CAPTURE_ID, "unix-stream-capture-v1");
        assert_eq!(EVENTS_SOCKET, "/streams/events.sock");
        assert_eq!(STDERR_SOCKET, "/streams/stderr.sock");
        assert_eq!(EVENTS_FILE, "/output/events.jsonl");
        assert_eq!(STDERR_FILE, "/output/qwen.stderr");
    }

    #[test]
    fn exclusive_output_creation_never_overwrites() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-capture-output-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&root).expect("create capture test root");
        let output = root.join("events.jsonl");
        create_private_output(&output).expect("create exclusive capture output");
        assert!(create_private_output(&output).is_err());
        std::fs::remove_dir_all(root).expect("remove capture test root");
    }

    #[tokio::test]
    async fn finalization_retains_both_outputs_flush_and_sync_failures() {
        let full = || {
            tokio::fs::File::from_std(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .expect("open Linux failing write device"),
            )
        };
        let mut events = full();
        let mut stderr = full();
        // Tokio admits each write to its blocking worker before reporting its
        // I/O outcome at flush. Both output owners must be observed.
        events.write_all(b"events").await.unwrap();
        stderr.write_all(b"stderr").await.unwrap();
        let error = sync_async_outputs(&mut events, &mut stderr)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            format!(
                "flush captured events: {space}; flush captured stderr: {space}; sync captured events: {invalid}; sync captured stderr: {invalid}",
                space = io::Error::from_raw_os_error(libc::ENOSPC),
                invalid = io::Error::from_raw_os_error(libc::EINVAL),
            )
        );
    }
}
