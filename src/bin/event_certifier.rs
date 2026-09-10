//! Artifact gate adapter for the service's production descriptor reader.

use std::path::Path;
use std::process::ExitCode;

use agent_service::result_parse::read_event_snapshot;

fn main() -> ExitCode {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if arguments.len() != 2 {
        eprintln!("usage: event_certifier EVENTS_JSONL | --contract-identity");
        return ExitCode::from(2);
    }
    if arguments[1] == "--contract-identity" {
        println!("{}", agent_service::stream_contract::STREAM_CONTRACT_SHA256);
        return ExitCode::SUCCESS;
    }
    match read_event_snapshot(Path::new(&arguments[1])) {
        Ok(Some(snapshot)) => {
            let (certificate, code) = match snapshot.certified {
                Ok(result) => (
                    serde_json::json!({"state": "certified", "result": result}),
                    ExitCode::SUCCESS,
                ),
                Err(error) => (
                    serde_json::json!({"state": "refused", "cause": error.to_string()}),
                    ExitCode::FAILURE,
                ),
            };
            println!(
                "{}",
                serde_json::json!({
                    "certification": certificate, "observed": snapshot.observed,
                    "replay_completion": snapshot.replay_completion,
                })
            );
            code
        }
        Ok(None) => {
            eprintln!("event_certifier: events file is absent");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("event_certifier: {error}");
            ExitCode::FAILURE
        }
    }
}
