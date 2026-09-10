//! Only registered fixture sessions may reach the production broker dispatcher.
//! The identical request bytes traverse its real bounded parser and handler.

#[path = "composition_fixture.rs"]
mod fixture;

use super::*;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;

struct RemoveFault {
    fixture: Arc<fixture::Fixture>,
    hits: std::sync::atomic::AtomicUsize,
    failure: std::sync::Mutex<Option<String>>,
}

static REMOVE_FAULT: std::sync::OnceLock<RemoveFault> = std::sync::OnceLock::new();

pub(super) fn before_owned_remove(
    name: &str,
    policy: &Policy,
    session_id: &str,
    component: Component,
    observed: &Value,
) -> Result<(), String> {
    let Some(fault) = REMOVE_FAULT.get() else {
        return Ok(());
    };
    if !matches!(component, Component::SessionCapture) {
        return Ok(());
    }
    let control = fault.fixture.root.join("control");
    let operation = || -> std::io::Result<bool> {
        if let Some(failure) = fault.failure.lock().unwrap().as_ref() {
            return Err(std::io::Error::other(failure.clone()));
        }
        if policy.runtime_root != fault.fixture.root.to_str().unwrap()
            || !fault.fixture.session_ids.iter().any(|id| id == session_id)
            || name != capture_name(session_id)
            || observed.pointer("/State/Running") != Some(&Value::Bool(false))
        {
            return Err(std::io::Error::other(
                "removal fault escaped its exact stopped capture owner",
            ));
        }
        if control.join("release-remove-fault").try_exists()? {
            return Ok(false);
        }
        if fault.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst) != 0 {
            return Err(std::io::Error::other(
                "capture removal fault reached more than once before release",
            ));
        }
        let bytes = serde_json::to_vec(&serde_json::json!({
            "v": 1, "site": "before_remove_stopped_capture", "session_id": session_id,
            "name": name, "observed": observed, "injected_errno": libc::EIO,
        }))?;
        publish_receipt(&control, "remove-refused.json", &bytes)?;
        Ok(true)
    };
    match operation() {
        Ok(false) => Ok(()),
        Ok(true) => Err(format!(
            "composition injected EIO before removing proved-stopped capture {name}: {}",
            std::io::Error::from_raw_os_error(libc::EIO)
        )),
        Err(error) => {
            *fault.failure.lock().unwrap() = Some(error.to_string());
            Err(format!("capture removal fixture failed: {error}"))
        }
    }
}

fn owned_request(bytes: &[u8], fixture: &fixture::Fixture) -> Result<(), String> {
    if bytes.len() as u64 > REQUEST_LIMIT {
        return Err("composition request exceeded the production bound".into());
    }
    let request = parse_request(bytes)?;
    let session_id = match request {
        Request::Preflight | Request::SweepOrphans => {
            return Err("composition gate forbids deployed-stack and global operations".into());
        }
        Request::CreateSession { session_id }
        | Request::ProveSessionQuiescent { session_id }
        | Request::SessionLogs { session_id }
        | Request::WaitSession { session_id }
        | Request::WaitAgentReady { session_id }
        | Request::WaitCaptureComplete { session_id }
        | Request::StopSession { session_id }
        | Request::RemoveSession { session_id } => session_id,
    };
    if !fixture.session_ids.contains(&session_id) {
        return Err(format!(
            "composition gate refuses unregistered session {session_id:?}"
        ));
    }
    Ok(())
}

fn publish_receipt(control: &std::path::Path, marker: &str, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = control.join(format!("{marker}.pending"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    std::io::Write::write_all(&mut file, bytes)?;
    file.sync_all()?;
    std::fs::hard_link(&temporary, control.join(marker))?;
    std::fs::File::open(control)?.sync_all()?;
    std::fs::remove_file(temporary)?;
    std::fs::File::open(control)?.sync_all()
}

async fn barrier(
    fixture: &fixture::Fixture,
    marker: &str,
    release: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    let control = fixture.root.join("control");
    publish_receipt(&control, marker, bytes)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    while !control.join(release).try_exists()? {
        if tokio::time::Instant::now() >= deadline {
            return Err(std::io::Error::other(format!(
                "barrier {marker} was not released"
            )));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}

async fn guarded_connection(
    mut stream: UnixStream,
    policy: Arc<Policy>,
    fixture: Arc<fixture::Fixture>,
    mutation_lock: Arc<tokio::sync::Mutex<()>>,
) -> Result<(), String> {
    let mut bytes = Vec::new();
    (&mut stream)
        .take(REQUEST_LIMIT + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| format!("gate request read: {e}"))?;
    owned_request(&bytes, &fixture)?;
    let hold_created = fixture.case == fixture::Case::CancelStartGate
        && matches!(parse_request(&bytes)?, Request::CreateSession { .. });
    println!(
        "COMPOSITION_BROKER_REQUEST {}",
        String::from_utf8_lossy(&bytes).trim_end()
    );
    if fixture.case == fixture::Case::CancelHeldSse
        && matches!(parse_request(&bytes)?, Request::RemoveSession { .. })
    {
        barrier(&fixture, "remove-held.json", "release-remove", &bytes)
            .await
            .map_err(|e| format!("before-remove barrier: {e}"))?;
    }
    let (mut forwarding, original) = UnixStream::pair().map_err(|e| e.to_string())?;
    let handler =
        tokio::spawn(async move { serve_connection(original, &policy, &mutation_lock).await });
    let exchange = async {
        forwarding.write_all(&bytes).await?;
        forwarding.shutdown().await?;
        let mut response = Vec::new();
        (&mut forwarding)
            .take(RESPONSE_LIMIT as u64 + 1)
            .read_to_end(&mut response)
            .await?;
        if response.len() > RESPONSE_LIMIT {
            return Err(std::io::Error::other(
                "production broker response exceeded its bound",
            ));
        }
        Ok(response)
    }
    .await;
    // Join the actual operation before withholding its observation. A lost
    // client response never abandons a mutation or Docker child.
    let joined = handler
        .await
        .map_err(|e| format!("broker handler join: {e}"))?;
    let response = match (exchange, joined) {
        (Ok(response), Ok(())) => response,
        (exchange, handler) => {
            return Err(format!(
                "gate exchange={exchange:?}; production handler={handler:?}"
            ));
        }
    };
    println!(
        "COMPOSITION_BROKER_RESPONSE {}",
        String::from_utf8_lossy(&response).trim_end()
    );
    let delivery = async {
        if hold_created {
            let result: Value = serde_json::from_slice(&response)?;
            if result.get("ok") != Some(&Value::Bool(true)) {
                return Err(std::io::Error::other(
                    "real topology creation failed before gate barrier",
                ));
            }
            barrier(
                &fixture,
                "created-held.json",
                "release-create-response",
                &response,
            )
            .await?;
            // This case deliberately closes the held response after the HTTP
            // cancellation is durable. The real create handler is still joined.
            println!("COMPOSITION_HELD_CREATE_RESPONSE_CLOSED");
            return Ok(());
        }
        let held_capture = matches!(
            fixture.case,
            fixture::Case::CaptureProofHeldCancel
                | fixture::Case::CaptureProofLost
                | fixture::Case::CaptureByteMismatch
                | fixture::Case::CaptureModeMismatch
        ) && matches!(
            parse_request(&bytes).map_err(std::io::Error::other)?,
            Request::WaitCaptureComplete { .. }
        );
        let lost_wait = fixture.case == fixture::Case::WaitObservationLost
            && matches!(
                parse_request(&bytes).map_err(std::io::Error::other)?,
                Request::WaitSession { .. }
            );
        let lost_quiescence = fixture.case == fixture::Case::QuiescenceObservationLost
            && !fixture
                .root
                .join("control/release-remove-fault")
                .try_exists()?
            && matches!(
                parse_request(&bytes).map_err(std::io::Error::other)?,
                Request::ProveSessionQuiescent { .. }
            );
        if held_capture || lost_wait || lost_quiescence {
            let result: Value = serde_json::from_slice(&response)?;
            if result.get("ok") != Some(&Value::Bool(true)) {
                return Err(std::io::Error::other(
                    "real broker observation failed before fault barrier",
                ));
            }
            let marker = if held_capture {
                "capture-proof-held.json"
            } else if lost_quiescence {
                "quiescence-observation-held.json"
            } else {
                "wait-observation-held.json"
            };
            barrier(&fixture, marker, "release-observation", &response).await?;
            if fixture.case == fixture::Case::CaptureProofLost || lost_wait || lost_quiescence {
                println!("COMPOSITION_OBSERVATION_CLOSED_AFTER_JOIN {marker}");
                return Ok(());
            }
        }
        stream.write_all(&response).await?;
        stream.shutdown().await
    }
    .await;
    delivery.map_err(|error| format!("gate response delivery after joined handler: {error}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the owned final-image composition harness"]
async fn final_image_broker() {
    let fixture = Arc::new(fixture::Fixture::read());
    if matches!(
        fixture.case,
        fixture::Case::RemoveStoppedCaptureFailed | fixture::Case::QuiescenceObservationLost
    ) {
        REMOVE_FAULT
            .set(RemoveFault {
                fixture: Arc::clone(&fixture),
                hits: std::sync::atomic::AtomicUsize::new(0),
                failure: std::sync::Mutex::new(None),
            })
            .unwrap_or_else(|_| panic!("removal fixture already installed"));
    }
    assert_eq!(unsafe { libc::getegid() }, 984);
    let mut policy: Policy = serde_json::from_str(POLICY_JSON).unwrap();
    policy.runtime_root = fixture.root.to_str().unwrap().into();
    policy.state_dir = fixture.root.join("state").to_str().unwrap().into();
    policy.model_socket_dir = fixture.root.join("model-socket").to_str().unwrap().into();
    policy.broker_socket_path = fixture
        .root
        .join("control/broker.sock")
        .to_str()
        .unwrap()
        .into();
    policy.agent.image_id = fixture.agent_image.clone();
    policy.relay.image_id = fixture.relay_image.clone();
    policy.capture.image_id = fixture.capture_image.clone();
    validate_policy(&policy).unwrap();
    let socket = PathBuf::from(&policy.broker_socket_path);
    validate_socket_parent(&socket).unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660)).unwrap();
    let policy = Arc::new(policy);
    let mutation_lock = Arc::new(tokio::sync::Mutex::new(()));
    let mut children = tokio::task::JoinSet::new();
    let mut failures = Vec::new();
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
    println!("COMPOSITION_BROKER_READY");
    loop {
        tokio::select! {
            signal = term.recv() => { assert!(signal.is_some()); break; }
            connected = listener.accept() => {
                let (stream, _) = connected.unwrap();
                children.spawn(guarded_connection(stream, Arc::clone(&policy), Arc::clone(&fixture), Arc::clone(&mutation_lock)));
            }
            result = children.join_next(), if !children.is_empty() => {
                match result.unwrap() {
                    Ok(Ok(())) => {},
                    failure => failures.push(format!("{failure:?}")),
                }
            }
        }
    }
    drop(listener);
    while let Some(result) = children.join_next().await {
        if !matches!(result, Ok(Ok(()))) {
            failures.push(format!("{result:?}"));
        }
    }
    std::fs::remove_file(socket).unwrap();
    assert!(
        failures.is_empty(),
        "broker operation failures: {failures:?}"
    );
    if let Some(fault) = REMOVE_FAULT.get() {
        assert_eq!(fault.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            fault.failure.lock().unwrap().is_none(),
            "removal fixture infrastructure failed"
        );
        assert!(
            fault
                .fixture
                .root
                .join("control/release-remove-fault")
                .is_file(),
            "expected removal was never reconciled"
        );
    }
    println!("COMPOSITION_BROKER_SETTLED");
}
