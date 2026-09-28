use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use runtime_contract::usage::ServedUsage;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;

mod compaction;
mod evidence;
mod framing;

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn served(prompt: u64, output: u64, thoughts: u64, cached: u64) -> Value {
    json!({"promptTokenCount":prompt,"candidatesTokenCount":output,
        "thoughtsTokenCount":thoughts,"cachedContentTokenCount":cached,
        "totalTokenCount":prompt + output})
}

fn ordinary_usage() -> Value {
    served(42, 9, 6, 0)
}

fn frames(rows: &[Value]) -> String {
    rows.iter().map(|row| format!("{row}\n")).collect()
}

/// Explicit physical evidence. Mutations operate on finished rows; no helper
/// infers requests from presentation or repairs a damaged terminal summary.
#[derive(Clone)]
struct Trace {
    rows: Vec<Value>,
    requests: Vec<(String, Option<Value>)>,
    last_model_text: BTreeMap<Option<String>, String>,
}

impl Trace {
    fn new() -> Self {
        let mut init: Value = serde_json::from_str(env!("CAPTURED_STREAM_BINDINGS_JSON")).unwrap();
        init.as_object_mut().unwrap().extend(
            json!({
                "type":"system","subtype":"init","uuid":"init","session_id":"a",
                "parent_tool_use_id":null
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        Self {
            rows: vec![
                json!({
                    "type":"system","subtype":"stream_start","uuid":"stream-start","session_id":"a",
                    "parent_tool_use_id":null,"stream_contract_sha256":init["stream_contract_sha256"],
                    "request_evidence_origin":{"journal_id":"fixture","first_sequence":1}
                }),
                init,
            ],
            requests: Vec::new(),
            last_model_text: BTreeMap::new(),
        }
    }

    fn push(&mut self, mut row: Value) {
        let object = row.as_object_mut().unwrap();
        object.insert("uuid".into(), json!(format!("event-{}", self.rows.len())));
        object.insert("session_id".into(), json!("a"));
        object.entry("parent_tool_use_id").or_insert(Value::Null);
        self.rows.push(row);
    }

    fn response(&mut self, request: &str, sequence: u64, event: Value) {
        self.push(json!({"type":"model_response","response":{
            "journal_id":"fixture","request_id":request,"sequence":sequence,"event":event}}));
    }

    /// A transport prefix without an outcome remains pending, even when its
    /// body is complete. Utility calls have no logical history disposition.
    fn pending(&mut self, scope: &str, usage: Option<&Value>) -> String {
        let sequence = self.requests.len() + 1;
        let id = format!("request-{sequence}");
        let body =
            json!({"model":"fixture","kv_scope":scope,"stream":false,"messages":[]}).to_string();
        let wire_usage = usage
            .filter(|value| {
                value["promptTokenCount"].as_u64().is_some()
                    && value["candidatesTokenCount"].as_u64().is_some()
                    && value["totalTokenCount"].as_u64().is_some()
                    && value["cachedContentTokenCount"].as_u64().is_some()
                    && value["thoughtsTokenCount"].as_u64().is_some()
            })
            .map(|value| {
                json!({
                    "prompt_tokens":value["promptTokenCount"],
                    "completion_tokens":value["candidatesTokenCount"],
                    "total_tokens":value["totalTokenCount"],
                    "prompt_tokens_details":{"cached_tokens":value["cachedContentTokenCount"]},
                    "completion_tokens_details":{"reasoning_tokens":value["thoughtsTokenCount"]}
                })
            });
        let response_body = json!({
            "id":"utility-reply","object":"chat.completion","created":1,
            "model":"fixture","choices":[{"index":0,
                "message":{"role":"assistant","content":"ok"},
                "finish_reason":"stop"}],
            "usage":wire_usage
        })
        .to_string();
        self.push(json!({"type":"model_request","request":{
            "journal_id":"fixture","sequence":sequence,"request_id":id,
            "owner":{"kind":"utility"},"kv_scope":scope,
            "segment_id":format!("segment-{sequence}"),"prompt_id":"fixture",
            "decode_policy":{"mode":"nonstream","model":"fixture",
                "strict_tool_calling":false,"named_tool_choice":null,
                "exact_token_counting":false,"tagged_thinking_tags":false},
            "body":{"kind":"full","json":body},"body_bytes":body.len(),"body_sha256":hash(body.as_bytes())}}));
        self.response(
            &id,
            1,
            json!({"kind":"http","status":200,"content_type":"application/json"}),
        );
        self.response(
            &id,
            2,
            json!({"kind":"body","offset":0,"base64":STANDARD.encode(&response_body)}),
        );
        self.response(
            &id,
            3,
            json!({"kind":"end","termination":"eof",
            "body_bytes":response_body.len(),"body_sha256":hash(response_body.as_bytes()),"error":null}),
        );
        self.requests.push((scope.into(), None));
        id
    }

    fn utility(&mut self, scope: &str, usage: Value) {
        let id = self.pending(scope, Some(&usage));
        self.response(
            &id,
            4,
            json!({"kind":"outcome","status":"completed","error":null,"served_usage":usage,
                "sdk_values_seen":1,"pipeline_outputs_delivered":1}),
        );
        self.requests.last_mut().unwrap().1 = Some(usage);
    }

    /// Rebind the captured request, seed, physical tool ID, generation and
    /// projection together, then seal their changed byte bodies. The raw tool
    /// argument lexemes remain captured. Runtime metadata comes from the
    /// service manifest in Trace::new.
    fn chat(&mut self, parent: Option<&str>, call_id: &str) {
        let captured: Vec<Value> = serde_json::from_str(include_str!(
            "../../protocol/engine/src/fixtures/ordinary-tool-wire.json"
        ))
        .unwrap();
        let original_session = captured[0]["session_id"].as_str().unwrap();
        let original_attempt = captured[1]["request"]["owner"]["attempt_id"]
            .as_str()
            .unwrap();
        let original_request = captured[1]["request"]["request_id"].as_str().unwrap();
        assert_ne!(call_id, "provider");
        let scope = parent.unwrap_or("a");
        let sequence = self.requests.len() + 1;
        let request_id = format!("request-{sequence}");
        let attempt_id = format!("attempt-{sequence}");
        let generation_id = format!("generation-{sequence}");
        let origin = json!({"kind":"model","attempt_id":attempt_id,"kv_scope":scope});
        let mut generation_hash = String::new();
        let mut response_body = None;
        let mut usage = None;
        for source in &captured {
            let mut row = source.clone();
            row["parent_tool_use_id"] = json!(parent);
            match source["type"].as_str().unwrap() {
                "system" | "result" => continue,
                "model_request" => {
                    row["parent_tool_use_id"] = Value::Null;
                    let evidence = &mut row["request"];
                    let body = evidence["body"]["json"]
                        .as_str()
                        .unwrap()
                        .replace(original_session, scope);
                    evidence["body"]["json"] = json!(body);
                    evidence["body_bytes"] = json!(body.len());
                    evidence["body_sha256"] = json!(hash(body.as_bytes()));
                    evidence["journal_id"] = json!("fixture");
                    evidence["sequence"] = json!(sequence);
                    evidence["request_id"] = json!(request_id);
                    evidence["owner"]["attempt_id"] = json!(attempt_id);
                    evidence["kv_scope"] = json!(scope);
                    evidence["segment_id"] = json!(format!("segment-{sequence}"));
                }
                "model_response" => {
                    row["parent_tool_use_id"] = Value::Null;
                    row["response"]["journal_id"] = json!("fixture");
                    row["response"]["request_id"] = json!(request_id);
                    let event = &mut row["response"]["event"];
                    match event["kind"].as_str().unwrap() {
                        "body" => {
                            assert!(response_body.is_none());
                            let raw = String::from_utf8(
                                STANDARD.decode(event["base64"].as_str().unwrap()).unwrap(),
                            )
                            .unwrap();
                            let provider_id = r#""id":"provider""#;
                            assert_eq!(raw.matches(provider_id).count(), 1);
                            let changed =
                                raw.replace(provider_id, &format!(r#""id":{}"#, json!(call_id)));
                            response_body = Some((changed.len(), hash(changed.as_bytes())));
                            event["base64"] = json!(STANDARD.encode(changed.as_bytes()));
                        }
                        "end" => {
                            let (size, digest) = response_body.as_ref().unwrap();
                            event["body_bytes"] = json!(size);
                            event["body_sha256"] = json!(digest);
                        }
                        "outcome" => {
                            usage = Some(event["served_usage"].clone());
                        }
                        _ => {}
                    }
                }
                "model_normalization_seed" => {
                    let seed = &mut row["normalization_seed"];
                    seed["journal_id"] = json!("fixture");
                    seed["origin"] = origin.clone();
                    seed["request_id"] = json!(request_id);
                }
                "model_generation" => {
                    let evidence = &mut row["generation"];
                    let envelope = evidence["generation_json"]
                        .as_str()
                        .unwrap()
                        .replace(original_session, scope)
                        .replace(original_attempt, &attempt_id)
                        .replace(original_request, &request_id)
                        .replace("provider__qwen_dup_2", call_id)
                        .replace("\"provider\"", &json!(call_id).to_string())
                        .replace(
                            "\"parent_tool_use_id\":null",
                            &format!("\"parent_tool_use_id\":{}", json!(parent)),
                        );
                    generation_hash = hash(envelope.as_bytes());
                    let parsed: Value = serde_json::from_str(&envelope).unwrap();
                    let mut display = String::new();
                    for observation in parsed["observations"].as_array().unwrap() {
                        let parts = observation["response"]["candidates"][0]["content"]["parts"]
                            .as_array()
                            .unwrap();
                        for part in parts {
                            if part["thought"] != true {
                                if let Some(text) = part["text"].as_str() {
                                    display.push_str(text);
                                }
                            }
                        }
                    }
                    self.last_model_text
                        .insert(parent.map(str::to_string), display);
                    evidence["journal_id"] = json!("fixture");
                    evidence["generation_id"] = json!(generation_id);
                    evidence["generation_bytes"] = json!(envelope.len());
                    evidence["generation_sha256"] = json!(generation_hash);
                    evidence["generation_json"] = json!(envelope);
                }
                "model_attempt_completion" => {
                    let completion = &mut row["completion"];
                    completion["journal_id"] = json!("fixture");
                    completion["origin"] = origin.clone();
                    completion["generation_id"] = json!(generation_id);
                    completion["generation_sha256"] = json!(generation_hash);
                    completion["request_ids"] = json!([request_id]);
                }
                "stream_event" => {
                    row["origin"] = origin.clone();
                    if row["event"]["type"] == "content_block_start" {
                        row["event"]["content_block"]["id"] = json!(call_id);
                    }
                }
                _ => panic!("unexpected captured fixture record"),
            }
            self.push(row);
        }
        self.requests.push((scope.into(), usage));
    }

    fn presentation(&mut self, parent: Option<&str>) {
        self.push(json!({"type":"assistant","parent_tool_use_id":parent,
            "origin":{"kind":"runtime"},"message":{"id":format!("notice-{}", self.rows.len()),
                "type":"message","role":"assistant","stop_reason":null,
                "content":[{"type":"text","text":"Runtime status."}],"usage":null}}));
    }

    fn summary(&self, scope: Option<&str>) -> Value {
        let mut requests = 0;
        let mut reports = 0;
        let mut pending = 0;
        let mut unknown = 0;
        let mut usage = served(0, 0, 0, 0);
        for (owner, outcome) in &self.requests {
            if scope.is_some_and(|scope| scope != owner) {
                continue;
            }
            requests += 1;
            match outcome {
                None => pending += 1,
                Some(Value::Null) => unknown += 1,
                Some(report) => {
                    reports += 1;
                    for (key, value) in usage.as_object_mut().unwrap() {
                        *value = json!(value.as_u64().unwrap() + report[key].as_u64().unwrap());
                    }
                }
            }
        }
        json!({"requests":requests,"usageReports":reports,"unfinalizedRequests":pending,
            "unreportedUsageRequests":unknown,"usage":if reports == 0 { Value::Null } else { usage }})
    }

    fn terminal(&mut self, parent: Option<&str>, turns: u64, error: Option<(&str, &str)>) {
        let result = self
            .last_model_text
            .get(&parent.map(str::to_string))
            .cloned()
            .unwrap_or_else(|| "ok".to_string());
        let mut row = json!({"type":"result","parent_tool_use_id":parent,
            "subtype":"success","is_error":false,"duration_ms":2,"duration_api_ms":1,
            "num_turns":turns,"result":result,"usage":self.summary(parent),"permission_denials":[]});
        if parent.is_none() {
            row["request_evidence"] = json!({"journal_id":"fixture","first_sequence":1,
                "request_count":self.requests.len(),"open_response_ids":[],"open_attempt_ids":[]});
        }
        if let Some((subtype, message)) = error {
            row["subtype"] = json!(subtype);
            row["is_error"] = json!(true);
            row["error"] = json!({"message":message});
            row.as_object_mut().unwrap().remove("result");
        }
        self.push(row);
    }

    fn text(&self) -> String {
        frames(&self.rows)
    }
    fn snapshot(&self) -> EventSnapshot {
        snapshot_text(&self.text()).unwrap()
    }
    fn certify(&self) -> AgentResult {
        self.snapshot()
            .certified
            .expect("positive fixture must certify")
    }

    fn ordinary() -> Self {
        let mut trace = Self::new();
        trace.utility("a", ordinary_usage());
        trace.terminal(None, 1, None);
        trace
    }

    fn delegated() -> Self {
        let mut trace = Self::new();
        trace.chat(None, "child");
        trace.utility("child", ordinary_usage());
        trace.terminal(
            Some("child"),
            3,
            Some(("error_during_execution", "MAX_TURNS")),
        );
        trace.terminal(None, 1, None);
        trace
    }
}

fn observed_usage(snapshot: &EventSnapshot) -> Value {
    serde_json::to_value(snapshot.observed.observed_usage).unwrap()
}

fn assert_refused_at(trace: &Trace, needle: &str) {
    let error = trace.snapshot().certified.unwrap_err().to_string();
    assert!(error.contains(needle), "expected {needle:?}: {error}");
}

fn snapshot_text(text: &str) -> ServiceResult<EventSnapshot> {
    snapshot_bytes(text.as_bytes())
}

fn snapshot_bytes(bytes: &[u8]) -> ServiceResult<EventSnapshot> {
    let path = std::env::temp_dir().join(format!(
        "agent-service-result-{}.jsonl",
        uuid::Uuid::new_v4()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(bytes).and_then(|_| file.sync_all()).unwrap();
    if unsafe { libc::geteuid() } == 0 {
        std::os::unix::fs::chown(&path, Some(1000), Some(1000)).unwrap();
    }
    let result =
        read_event_snapshot(&path).map(|snapshot| snapshot.expect("test event file exists"));
    std::fs::remove_file(path).unwrap();
    result
}
