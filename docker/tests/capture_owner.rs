//! CPU capture-owner qualification. The deployed binary has none of these hooks.

use super::*;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const STDERR_PREFIX: &[u8] = b"stderr-prefix\n";
const STDERR_TAIL: &[u8] = "stderr-tail:π\n".as_bytes();

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Case {
    DelayedStderrAndLastSync,
    StderrSyncEio,
    EventsCopyFailure,
    EventsCopyFailureAndStderrSyncEio,
}

impl Case {
    fn copy_failure(self) -> bool {
        matches!(
            self,
            Self::EventsCopyFailure | Self::EventsCopyFailureAndStderrSyncEio
        )
    }

    fn sync_failure(self) -> bool {
        matches!(
            self,
            Self::StderrSyncEio | Self::EventsCopyFailureAndStderrSyncEio
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    v: u32,
    case: Case,
    events_sha256: String,
    events_bytes: usize,
    copy_limit_bytes: Option<u64>,
}

#[derive(Default)]
struct Audit {
    receipts: Vec<Value>,
    failure: Option<String>,
}

struct Owner {
    fixture: Fixture,
    audit: Mutex<Audit>,
    ready: Notify,
    stderr_copied: Notify,
    cancel: CancellationToken,
}

static OWNER: OnceLock<Arc<Owner>> = OnceLock::new();

fn record(mut value: Value) -> io::Result<()> {
    let Some(owner) = OWNER.get() else {
        return Ok(());
    };
    let mut audit = owner.audit.lock().unwrap();
    if let Some(failure) = &audit.failure {
        return Err(io::Error::other(failure.clone()));
    }
    value["sequence"] = json!(audit.receipts.len() + 1);
    let result = (|| {
        let mut output = std::io::stdout().lock();
        output.write_all(b"CAPTURE_OWNER_RECEIPT ")?;
        serde_json::to_writer(&mut output, &value)?;
        output.write_all(b"\n")?;
        output.flush()
    })();
    match result {
        Ok(()) => {
            audit.receipts.push(value);
            Ok(())
        }
        Err(error) => {
            audit.failure = Some(format!("capture fixture receipt failed: {error}"));
            Err(error)
        }
    }
}

fn io_observation<T>(result: &io::Result<T>) -> Value {
    match result {
        Ok(_) => json!({"state": "returned", "ok": true}),
        Err(error) => json!({"state": "returned", "ok": false,
            "errno": error.raw_os_error(), "cause": error.to_string()}),
    }
}

pub(super) fn ready() -> Result<(), String> {
    record(json!({"site": "ready"})).map_err(|error| error.to_string())?;
    if let Some(owner) = OWNER.get() {
        owner.ready.notify_one();
    }
    Ok(())
}

pub(super) fn after_copy(label: &str, result: &io::Result<u64>) -> Result<(), String> {
    let mut receipt = json!({"site": "after_copy", "output": label,
        "operation": io_observation(result)});
    if let Ok(bytes) = result {
        receipt["bytes"] = json!(bytes);
    }
    record(receipt).map_err(|error| error.to_string())?;
    if label == "stderr" && result.is_ok() {
        if let Some(owner) = OWNER.get() {
            owner.stderr_copied.notify_one();
        }
    }
    Ok(())
}

async fn release(owner: &Owner, name: &str) -> io::Result<()> {
    let path = Path::new("/gate-control").join(name);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file()
                    || metadata.uid() != 1000
                    || metadata.gid() != 1000
                    || metadata.permissions().mode() & 0o7777 != 0o600
                {
                    return Err(io::Error::other("capture release ownership drift"));
                }
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        tokio::select! {
            () = owner.cancel.cancelled() => return Err(io::Error::other("capture fixture control cancelled")),
            () = tokio::time::sleep_until(deadline) => return Err(io::Error::other(format!("capture release {name} timed out"))),
            () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {},
        }
    }
}

pub(super) async fn before_io(operation: &str, label: &str) -> io::Result<()> {
    let Some(owner) = OWNER.get() else {
        return Ok(());
    };
    let injected = operation == "sync" && label == "stderr" && owner.fixture.case.sync_failure();
    record(
        json!({"site": "before_io", "output": label, "operation": operation,
        "syscall_attempted": false, "injected_errno": if injected { Some(libc::EIO) } else { None }}),
    )?;
    if operation == "sync"
        && label == "stderr"
        && owner.fixture.case == Case::DelayedStderrAndLastSync
    {
        release(owner, "release-sync").await?;
        record(json!({"site": "released", "barrier": "last_sync"}))?;
    }
    if injected {
        Err(io::Error::from_raw_os_error(libc::EIO))
    } else {
        Ok(())
    }
}

pub(super) fn after_io(operation: &str, label: &str, result: &io::Result<()>) -> io::Result<()> {
    record(
        json!({"site": "after_io", "output": label, "operation": operation,
        "result": io_observation(result)}),
    )
}

pub(super) fn before_complete(events: u64, stderr: u64) -> Result<(), String> {
    record(json!({"site": "before_complete", "events_bytes": events, "stderr_bytes": stderr}))
        .map_err(|error| error.to_string())
}

struct FileLimit {
    previous: libc::rlimit,
    signal: libc::sigaction,
    restored: bool,
}

impl FileLimit {
    fn install(limit: u64) -> io::Result<Self> {
        let mut previous = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        let mut signal = std::mem::MaybeUninit::<libc::sigaction>::uninit();
        if unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, previous.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::sigaction(libc::SIGXFSZ, std::ptr::null(), signal.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let previous = unsafe { previous.assume_init() };
        let signal = unsafe { signal.assume_init() };
        if limit >= previous.rlim_cur {
            return Err(io::Error::other(
                "fixture must lower the existing soft file limit",
            ));
        }
        let guard = Self {
            previous,
            signal,
            restored: false,
        };
        let mut ignored: libc::sigaction = unsafe { std::mem::zeroed() };
        ignored.sa_sigaction = libc::SIG_IGN;
        if unsafe { libc::sigemptyset(&mut ignored.sa_mask) } != 0
            || unsafe { libc::sigaction(libc::SIGXFSZ, &ignored, std::ptr::null_mut()) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let limited = libc::rlimit {
            rlim_cur: limit,
            rlim_max: previous.rlim_max,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &limited) } != 0 {
            return Err(io::Error::last_os_error());
        }
        record(
            json!({"site": "file_limit_installed", "soft_before": previous.rlim_cur,
            "hard": previous.rlim_max, "soft": limit, "sigxfsz_handler_before": signal.sa_sigaction,
            "sigxfsz_handler": ignored.sa_sigaction}),
        )?;
        // Setup failures above drop and restore this process-local guard.
        Ok(guard)
    }

    fn restore(&mut self) -> io::Result<()> {
        let limit = unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &self.previous) };
        let limit_error = (limit != 0).then(io::Error::last_os_error);
        let signal = unsafe { libc::sigaction(libc::SIGXFSZ, &self.signal, std::ptr::null_mut()) };
        let signal_error = (signal != 0).then(io::Error::last_os_error);
        self.restored = limit_error.is_none() && signal_error.is_none();
        if self.restored {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "restore file limit={limit_error:?}; signal={signal_error:?}"
            )))
        }
    }
}

impl Drop for FileLimit {
    fn drop(&mut self) {
        if !self.restored {
            if let Err(error) = self.restore() {
                eprintln!("capture fixture restoration failed: {error}");
            }
        }
    }
}

async fn writers(owner: Arc<Owner>, events: Vec<u8>) -> Result<(), String> {
    tokio::select! {
        () = owner.ready.notified() => {},
        () = owner.cancel.cancelled() => return Err("writers cancelled before capture ready".into()),
    }
    let mut event_socket = UnixStream::connect(EVENTS_SOCKET)
        .await
        .map_err(|e| e.to_string())?;
    let mut stderr_socket = UnixStream::connect(STDERR_SOCKET)
        .await
        .map_err(|e| e.to_string())?;
    stderr_socket
        .write_all(STDERR_PREFIX)
        .await
        .map_err(|e| e.to_string())?;
    if owner.fixture.case.copy_failure() {
        stderr_socket
            .write_all(STDERR_TAIL)
            .await
            .map_err(|e| e.to_string())?;
        stderr_socket.shutdown().await.map_err(|e| e.to_string())?;
        tokio::select! {
            () = owner.stderr_copied.notified() => {},
            () = owner.cancel.cancelled() => return Err("writers cancelled before stderr copy settled".into()),
        }
    }
    let mut accepted = 0;
    let mut write_failure = None;
    while accepted < events.len() {
        match event_socket.write(&events[accepted..]).await {
            Ok(0) => return Err("fixture events socket returned a zero-length write".into()),
            Ok(bytes) => accepted += bytes,
            Err(error) => {
                write_failure = Some(("write", error));
                break;
            }
        }
    }
    if write_failure.is_none() {
        if let Err(error) = event_socket.shutdown().await {
            write_failure = Some(("shutdown", error));
        }
    }
    let events_outcome = match write_failure {
        None => json!({"state": "complete", "accepted_bytes": accepted}),
        Some((phase, error))
            if owner.fixture.case.copy_failure()
                && matches!(error.raw_os_error(), Some(libc::EPIPE | libc::ECONNRESET)) =>
        {
            json!({"state": "peer_closed", "phase": phase, "accepted_bytes": accepted,
                "errno": error.raw_os_error(), "cause": error.to_string()})
        }
        Some(("shutdown", error))
            if owner.fixture.case.copy_failure()
                && error.raw_os_error() == Some(libc::ENOTCONN) =>
        {
            json!({"state": "peer_closed", "phase": "shutdown", "accepted_bytes": accepted,
                "errno": error.raw_os_error(), "cause": error.to_string()})
        }
        Some((phase, error)) => {
            return Err(format!(
                "fixture events {phase} after {accepted} accepted bytes: {error}"
            ))
        }
    };
    if !owner.fixture.case.copy_failure() {
        if owner.fixture.case == Case::DelayedStderrAndLastSync {
            release(&owner, "release-stderr")
                .await
                .map_err(|e| e.to_string())?;
            record(json!({"site": "released", "barrier": "stderr_eof"}))
                .map_err(|e| e.to_string())?;
        }
        stderr_socket
            .write_all(STDERR_TAIL)
            .await
            .map_err(|e| e.to_string())?;
        stderr_socket.shutdown().await.map_err(|e| e.to_string())?;
    }
    record(
        json!({"site": "writers_finished", "events": events_outcome, "events_offered_bytes": events.len(),
        "stderr_bytes": STDERR_PREFIX.len() + STDERR_TAIL.len()}),
    )
    .map_err(|e| e.to_string())
}

fn audit(owner: &Owner, body: &Result<(), String>) -> Result<Vec<Value>, String> {
    let state = owner.audit.lock().unwrap();
    if let Some(failure) = &state.failure {
        return Err(failure.clone());
    }
    let receipts = &state.receipts;
    let count = |site: &str| receipts.iter().filter(|r| r["site"] == site).count();
    if count("ready") != 1 || count("writers_finished") != 1 || count("after_copy") != 2 {
        return Err("capture fixture did not observe both writers and copies exactly once".into());
    }
    let writer = receipts
        .iter()
        .find(|r| r["site"] == "writers_finished")
        .unwrap();
    let events = &writer["events"];
    let accepted = events["accepted_bytes"]
        .as_u64()
        .ok_or("writer accepted-byte observation is missing")?;
    if accepted > owner.fixture.events_bytes as u64
        || writer["events_offered_bytes"] != owner.fixture.events_bytes
        || writer["stderr_bytes"] != STDERR_PREFIX.len() + STDERR_TAIL.len()
        || (events["state"] == "complete" && accepted != owner.fixture.events_bytes as u64)
        || (events["state"] == "peer_closed" && !owner.fixture.case.copy_failure())
    {
        return Err(
            "writer completion disagrees with the declared input or copy-failure case".into(),
        );
    }
    for output in ["events", "stderr"] {
        let copies = receipts
            .iter()
            .filter(|r| r["site"] == "after_copy" && r["output"] == output)
            .collect::<Vec<_>>();
        if copies.len() != 1 {
            return Err(format!(
                "capture copy observation repeated/missing: {output}"
            ));
        }
        let expected_error = output == "events" && owner.fixture.case.copy_failure();
        if copies[0]["operation"]["ok"] != !expected_error {
            return Err(format!("capture copy outcome differs: {output}"));
        }
        if !expected_error {
            let bytes = if output == "events" {
                owner.fixture.events_bytes
            } else {
                STDERR_PREFIX.len() + STDERR_TAIL.len()
            };
            if copies[0]["bytes"] != bytes {
                return Err(format!("capture copy byte count differs: {output}"));
            }
        }
    }
    let actual_io = receipts
        .iter()
        .filter(|r| r["site"] == "before_io" || r["site"] == "after_io")
        .map(|r| {
            (
                r["site"].clone(),
                r["operation"].clone(),
                r["output"].clone(),
            )
        })
        .collect::<Vec<_>>();
    let mut expected_io = Vec::new();
    for (operation, output) in [
        ("flush", "events"),
        ("flush", "stderr"),
        ("sync", "events"),
        ("sync", "stderr"),
    ] {
        expected_io.push((json!("before_io"), json!(operation), json!(output)));
        if !(operation == "sync" && output == "stderr" && owner.fixture.case.sync_failure()) {
            expected_io.push((json!("after_io"), json!(operation), json!(output)));
        }
    }
    if actual_io != expected_io
        || receipts
            .iter()
            .any(|r| r["site"] == "after_io" && r["result"]["ok"] != true)
    {
        return Err(
            "capture finalizers did not return in their required order with the planned outcomes"
                .into(),
        );
    }
    let expected_releases = if owner.fixture.case == Case::DelayedStderrAndLastSync {
        2
    } else {
        0
    };
    if count("released") != expected_releases
        || count("file_limit_installed") != usize::from(owner.fixture.case.copy_failure())
        || count("file_limit_restored") != usize::from(owner.fixture.case.copy_failure())
    {
        return Err("capture fixture control/limit lifecycle differs".into());
    }
    let injected = receipts
        .iter()
        .filter(|r| r["injected_errno"] == libc::EIO)
        .count();
    if injected != usize::from(owner.fixture.case.sync_failure()) {
        return Err("capture injected fault consumption differs".into());
    }
    let expected_success = owner.fixture.case == Case::DelayedStderrAndLastSync;
    if body.is_ok() != expected_success || count("before_complete") != usize::from(expected_success)
    {
        return Err(format!(
            "capture body outcome differs from its case: {body:?}"
        ));
    }
    if owner.fixture.case.copy_failure() {
        let copy = receipts
            .iter()
            .find(|r| r["site"] == "after_copy" && r["output"] == "events")
            .unwrap();
        if copy["operation"]["errno"] != libc::EFBIG
            || !body
                .as_ref()
                .unwrap_err()
                .contains("capture events stream: File too large (os error 27)")
        {
            return Err("capture copy did not fail with real EFBIG".into());
        }
    }
    if owner.fixture.case.sync_failure()
        && !body
            .as_ref()
            .unwrap_err()
            .contains("sync captured stderr: Input/output error (os error 5)")
    {
        return Err("capture lost the independent sync EIO".into());
    }
    Ok(receipts.clone())
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires the owned capture-image fixture"]
async fn final_image_capture_owner() -> Result<(), String> {
    let fixture: Fixture = serde_json::from_slice(
        &std::fs::read("/gate/capture-fixture.json").map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let events = std::fs::read("/gate/events.jsonl").map_err(|e| e.to_string())?;
    if fixture.v != 1
        || events.len() != fixture.events_bytes
        || events.is_empty()
        || Sha256::digest(&events)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            != fixture.events_sha256
    {
        return Err("capture fixture certified input identity differs".into());
    }
    if fixture.case.copy_failure() != fixture.copy_limit_bytes.is_some()
        || fixture.copy_limit_bytes.is_some_and(|n| {
            n <= (STDERR_PREFIX.len() + STDERR_TAIL.len()) as u64 || n >= events.len() as u64
        })
    {
        return Err(
            "capture file limit cannot distinguish full stderr from oversized real events".into(),
        );
    }
    let owner = Arc::new(Owner {
        fixture,
        audit: Mutex::new(Audit::default()),
        ready: Notify::new(),
        stderr_copied: Notify::new(),
        cancel: CancellationToken::new(),
    });
    OWNER
        .set(Arc::clone(&owner))
        .map_err(|_| "capture owner already installed")?;
    let mut limit = owner
        .fixture
        .copy_limit_bytes
        .map(FileLimit::install)
        .transpose()
        .map_err(|e| e.to_string())?;
    let mut writer = tokio::spawn(writers(Arc::clone(&owner), events));
    let body = run_body();
    tokio::pin!(body);
    let mut failures = Vec::new();
    let mut writer_result = None;
    let result = tokio::select! {
        result = &mut body => result,
        result = &mut writer => {
            if !matches!(result, Ok(Ok(()))) {
                owner.cancel.cancel();
                if unsafe { libc::kill(libc::getpid(), libc::SIGTERM) } != 0 {
                    failures.push(format!("terminate capture after writer failure: {}", io::Error::last_os_error()));
                }
            }
            writer_result = Some(result);
            body.await
        }
    };
    owner.cancel.cancel();
    let joined = match writer_result {
        Some(result) => result,
        None => writer.await,
    };
    if !matches!(joined, Ok(Ok(()))) {
        failures.push(format!(
            "capture writers did not settle successfully: {joined:?}"
        ));
    }
    if let Some(limit) = &mut limit {
        if let Err(error) = limit.restore() {
            failures.push(error.to_string());
        } else {
            if let Err(error) = record(json!({"site": "file_limit_restored"})) {
                failures.push(error.to_string());
            }
        }
    }
    let receipts = match audit(&owner, &result) {
        Ok(receipts) => receipts,
        Err(error) => {
            failures.push(format!("capture audit: {error}"));
            return Err(format!("capture body={result:?}; {}", failures.join("; ")));
        }
    };
    if !failures.is_empty() {
        return Err(format!("capture body={result:?}; {}", failures.join("; ")));
    }
    let outcome = match result {
        Ok(()) => json!({"state": "ok"}),
        Err(cause) => json!({"state": "error", "cause": cause}),
    };
    let mut output = std::io::stdout().lock();
    output
        .write_all(b"CAPTURE_OWNER_OUTCOME ")
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(
        &mut output,
        &json!({"v": 1, "outcome": outcome, "writers": "joined", "receipts": receipts}),
    )
    .map_err(|e| e.to_string())?;
    output.write_all(b"\n").map_err(|e| e.to_string())?;
    output.flush().map_err(|e| e.to_string())
}
