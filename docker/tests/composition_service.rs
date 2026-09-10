//! Test bootstrap for the actual HTTP/session/publication path. It deliberately
//! does not run deployed-stack preflight, which owns the real GPU deployment.

#[path = "composition_fixture.rs"]
mod fixture;

use std::sync::Arc;

#[test]
#[ignore = "requires the owned final-image composition harness"]
fn certify_available_stream() {
    let fixture = fixture::Fixture::read();
    assert_eq!(fixture.session_ids.len(), 1);
    let path = fixture
        .root
        .join("state/sessions")
        .join(&fixture.session_ids[0])
        .join("output/events.jsonl");
    let snapshot = agent_service::result_parse::read_event_snapshot(&path)
        .expect("read actual captured stream")
        .expect("captured stream exists");
    let result = snapshot.certified
        .expect("available stream satisfies the production protocol certifier");
    println!("COMPOSITION_AVAILABLE_CERTIFICATE {}", serde_json::to_string(&result).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the owned final-image composition harness"]
async fn final_image_service() {
    let fixture = fixture::Fixture::read();
    let terminal_io = fixture.case.has_terminal_faults().then(|| {
        crate::runtime::terminal_io_test::install(
            &fixture.root,
            &fixture.root.join("state/composition-faults"),
        )
    });
    let mut lock: crate::config::StackLock =
        serde_json::from_str(crate::config::STACK_LOCK_JSON).unwrap();
    lock.service.runtime_root = fixture.root.to_str().unwrap().into();
    lock.service.state_dir = fixture.root.join("state").to_str().unwrap().into();
    lock.service.results_dir = fixture.root.join("results").to_str().unwrap().into();
    lock.broker.socket_path = fixture
        .root
        .join("control/broker.sock")
        .to_str()
        .unwrap()
        .into();
    lock.relay.model_socket_dir = fixture.root.join("model-socket").to_str().unwrap().into();
    lock.agent.image_id = fixture.agent_image;
    lock.relay.image_id = fixture.relay_image;
    lock.capture.image_id = fixture.capture_image;
    let cfg = Arc::new(crate::config::Config {
        listen_addr: lock.service.listen.parse().unwrap(),
        state_dir: lock.service.state_dir.clone().into(),
        results_dir: lock.service.results_dir.clone().into(),
        broker_socket: lock.broker.socket_path.clone().into(),
        model_socket: fixture.root.join("model-socket/relay.sock"),
        agent_image: lock.agent.image_tag.clone(),
        vllm_model_name: lock.backend.served_model.clone(),
        vllm_endpoint: lock.backend.endpoint.clone(),
        lock,
    });
    crate::init_tracing().unwrap();
    crate::bundle::check_host_dependencies().await.unwrap();
    let recovery = async {
        crate::runtime::recover_interrupted_deletions(&cfg).await?;
        crate::api::recover_local_state(&cfg).await
    }
    .await;
    if let Err(error) = recovery {
        // Expected refusal is a returned startup outcome. Complete the fault
        // audit normally before reporting it; unwinding skips that audit.
        drop(terminal_io);
        println!(
            "COMPOSITION_SERVICE_STARTUP_REFUSED {}",
            serde_json::json!({"cause": error.to_string()})
        );
        return;
    }
    let manager = Arc::new(crate::runtime::Manager::new(Arc::clone(&cfg)));
    let router = crate::api::router(crate::api::AppState {
        cfg: Arc::clone(&cfg),
        manager: Arc::clone(&manager),
    });
    let listener = tokio::net::TcpListener::bind(cfg.listen_addr)
        .await
        .unwrap();
    assert!(listener.local_addr().unwrap().ip().is_loopback());
    let signals = crate::install_shutdown_signals().unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let serve =
        axum::serve(listener, router).with_graceful_shutdown(shutdown.clone().cancelled_owned());
    let serving = tokio::spawn(async move { serve.await });
    println!(
        "COMPOSITION_SERVICE_READY contract={} case={:?}",
        agent_service::stream_contract::STREAM_CONTRACT_SHA256,
        fixture.case,
    );
    crate::wait_for_signal(signals).await;
    shutdown.cancel();
    let (sessions, http) = tokio::join!(manager.shutdown(), serving);
    sessions.expect("all actual session supervisors must settle");
    http.unwrap().expect("HTTP connections must settle");
    drop(terminal_io);
    println!("COMPOSITION_SERVICE_SETTLED");
}
