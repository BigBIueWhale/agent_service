use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use runtime_contract::usage::ServedUsage;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;

mod compaction;
mod display;
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

/// The provider's OpenAI-compatible report of a served usage.
fn wire_usage(usage: &Value) -> Value {
    json!({
        "prompt_tokens":usage["promptTokenCount"],
        "completion_tokens":usage["candidatesTokenCount"],
        "total_tokens":usage["totalTokenCount"],
        "prompt_tokens_details":{"cached_tokens":usage["cachedContentTokenCount"]},
        "completion_tokens_details":{"reasoning_tokens":usage["thoughtsTokenCount"]}
    })
}

fn frames(rows: &[Value]) -> String {
    rows.iter().map(|row| format!("{row}\n")).collect()
}

/// The decode policy every recorded request states: strict tool calling and
/// exact token counting are constant in the amended contract.
fn decode_policy(mode: &str, model: &str) -> Value {
    json!({"mode":mode,"model":model,"strict_tool_calling":true,
        "named_tool_choice":null,"exact_token_counting":true,"tagged_thinking_tags":false})
}

/// The last chat request of one kv_scope, from which the next one may be a delta.
#[derive(Clone)]
struct Invocation {
    request_id: String,
    segment_id: String,
    messages: Vec<String>,
    /// The request message the model's settled turn becomes in the next request.
    turn: String,
}

/// Explicit physical evidence. Mutations operate on finished rows; no helper
/// infers requests from presentation or repairs a damaged terminal summary.
#[derive(Clone)]
struct Trace {
    rows: Vec<Value>,
    /// Billed model_request records by kv_scope with their served outcome:
    /// None pending, Some(null) unreported, Some(usage) reported.
    requests: Vec<(String, Option<Value>)>,
    /// Every journal request (model_request and model_utility_request).
    physical: usize,
    model: String,
    last_model_text: BTreeMap<Option<String>, String>,
    chats: BTreeMap<String, Invocation>,
    last_request: BTreeMap<String, String>,
    /// Request messages carrying each user row a display scope showed since
    /// its previous chat request, in display order.
    displayed: BTreeMap<Option<String>, Vec<String>>,
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
            model: init["model"].as_str().unwrap().to_string(),
            rows: vec![
                json!({
                    "type":"system","subtype":"stream_start","uuid":"stream-start","session_id":"a",
                    "parent_tool_use_id":null,"stream_contract_sha256":init["stream_contract_sha256"],
                    "request_evidence_origin":{"journal_id":"fixture","first_sequence":1}
                }),
                init,
            ],
            requests: Vec::new(),
            physical: 0,
            last_model_text: BTreeMap::new(),
            chats: BTreeMap::new(),
            last_request: BTreeMap::new(),
            displayed: BTreeMap::new(),
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

    /// One decoded observation the client delivered, recorded in bytes.
    fn decoded(&mut self, request: &str, sequence: u64, observation: &Value) {
        let bytes = observation.to_string();
        self.response(
            request,
            sequence,
            json!({"kind":"decoded_body","role":"utility","index":0,"offset":0,
                "base64":STANDARD.encode(bytes.as_bytes())}),
        );
        self.response(
            request,
            sequence + 1,
            json!({"kind":"decoded_end","role":"utility","index":0,
                "body_bytes":bytes.len(),"body_sha256":hash(bytes.as_bytes())}),
        );
    }

    fn next_sequence(&mut self) -> usize {
        self.physical += 1;
        self.physical
    }

    /// A transport prefix without an outcome remains pending, even when its
    /// body is complete. Utility calls have no logical history disposition.
    fn pending(&mut self, scope: &str, usage: Option<&Value>) -> String {
        let sequence = self.next_sequence();
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
            .map(wire_usage);
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
            "owner":{"kind":"utility","operation_id":format!("operation-{sequence}"),"purpose":"other"},
            "kv_scope":scope,"segment_id":format!("segment-{sequence}"),"prompt_id":"fixture",
            "decode_policy":decode_policy("nonstream","fixture"),
            "body":{"kind":"full","json":body},"body_bytes":body.len(),"body_sha256":hash(body.as_bytes())}}));
        self.last_request.insert(scope.into(), id.clone());
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

    /// A completed utility request whose one decoded output the client delivered.
    /// A completed generation always serves its usage; a request whose usage
    /// stays unknown is one that did not complete (`unserved`).
    fn utility(&mut self, scope: &str, usage: Value) {
        assert!(!usage.is_null(), "a completed generation serves its usage; use unserved");
        let id = self.pending(scope, Some(&usage));
        self.decoded(
            &id,
            4,
            &json!({"response":{"candidates":[{"content":{"parts":[{"text":"ok"}],"role":"model"},
                "index":0,"finishReason":"STOP"}]},
                "incomplete_tool_calls":[],"tool_call_preparations":[]}),
        );
        self.response(
            &id,
            6,
            json!({"kind":"outcome","status":"completed","error":null,"served_usage":usage,
                "sdk_values_seen":1,"pipeline_outputs_delivered":1}),
        );
        self.response(&id, 7, json!({"kind":"delivery","outputs_delivered":1}));
        self.requests.last_mut().unwrap().1 = Some(usage);
    }

    /// A utility generation whose processing failed after a body that served
    /// no usage: finalized, delivering nothing, its usage unknown.
    fn unserved(&mut self, scope: &str) {
        let id = self.pending(scope, None);
        self.response(
            &id,
            4,
            json!({"kind":"outcome","status":"failed","error":"conversion failed",
                "served_usage":null,"sdk_values_seen":1,"pipeline_outputs_delivered":0}),
        );
        self.response(&id, 5, json!({"kind":"delivery","outputs_delivered":0}));
        self.requests.last_mut().unwrap().1 = Some(Value::Null);
    }

    /// A tool result the client displayed in `scope`; the scope's next chat
    /// request carries it as the tool message for its call.
    fn tool_result(&mut self, scope: Option<&str>, id: &str, text: &str) {
        self.push(json!({"type":"user","parent_tool_use_id":scope,
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,
                "content":text,"is_error":false}]}}));
        self.displayed
            .entry(scope.map(str::to_string))
            .or_default()
            .push(json!({"role":"tool","tool_call_id":id,"content":text}).to_string());
    }

    /// A notice the client displayed as user input in `scope`; the scope's
    /// next chat request carries it as a user message.
    fn notice(&mut self, scope: Option<&str>, text: &str) {
        self.push(json!({"type":"user","parent_tool_use_id":scope,
            "message":{"role":"user","content":[{"type":"text","text":text}]}}));
        self.displayed
            .entry(scope.map(str::to_string))
            .or_default()
            .push(json!({"role":"user","content":text}).to_string());
    }

    fn chat(&mut self, parent: Option<&str>, call_id: &str) {
        self.chat_with(parent, Some(call_id), false);
    }

    /// A continuing chat whose request is a delta on the scope's previous one.
    fn chat_delta(&mut self, parent: Option<&str>, call_id: &str) {
        self.chat_with(parent, Some(call_id), true);
    }

    /// A final text answer: the captured turn with its call removed from the
    /// physical body and from the generation, so it issues no tool.
    fn answer(&mut self, parent: Option<&str>, delta: bool) {
        self.chat_with(parent, None, delta);
    }

    /// Rebind the captured request, seed, physical tool ID, generation,
    /// display and partials together, then seal their changed byte bodies. The
    /// raw tool argument lexemes remain captured. Runtime metadata comes from
    /// the service manifest in Trace::new. The request carries the scope's
    /// history and every user row it displayed since its previous request.
    fn chat_with(&mut self, parent: Option<&str>, call: Option<&str>, delta: bool) {
        let call_id = call.unwrap_or("answer-without-call");
        let captured: Vec<Value> = serde_json::from_str(include_str!(
            "../../protocol/test-vectors/ordinary-tool-wire.json"
        ))
        .unwrap();
        let original_session = captured[0]["session_id"].as_str().unwrap();
        let original_attempt = captured[1]["request"]["owner"]["attempt_id"]
            .as_str()
            .unwrap();
        let original_request = captured[1]["request"]["request_id"].as_str().unwrap();
        assert_ne!(call_id, "provider");
        let scope = parent.unwrap_or("a");
        let sequence = self.physical + 1;
        let request_id = format!("request-{sequence}");
        let attempt_id = format!("attempt-{sequence}");
        let generation_id = format!("generation-{sequence}");
        let origin = json!({"kind":"model","attempt_id":attempt_id,"kv_scope":scope});
        let mut generation_hash = String::new();
        let mut response_body = None;
        let mut usage = None;
        // The captured body is one user message between a prefix and a suffix.
        let template = captured[1]["request"]["body"]["json"]
            .as_str()
            .unwrap()
            .replace(original_session, scope);
        let first = r#"{"role":"user","content":[{"type":"text","text":"work"}]}"#;
        let start = template.find("\"messages\":[").unwrap() + "\"messages\":[".len();
        assert!(template[start..].starts_with(&format!("{first}]")));
        let (prefix, suffix) = (
            template[..start].to_string(),
            template[start + first.len()..].to_string(),
        );
        let carried = self
            .displayed
            .remove(&parent.map(str::to_string))
            .unwrap_or_default();
        let previous = self.chats.get(scope).cloned();
        let (base, retained) = match &previous {
            Some(previous) => {
                let mut messages = previous.messages.clone();
                messages.push(previous.turn.clone());
                (messages, previous.messages.len())
            }
            None => (vec![first.to_string()], 0),
        };
        let mut messages = base.clone();
        messages.extend(carried.iter().cloned());
        let body_json = format!("{prefix}{}{suffix}", messages.join(","));
        let (body, segment_id) = if delta {
            let previous = previous.as_ref().expect("a delta continues its scope's chat");
            assert_eq!(
                self.last_request.get(scope),
                Some(&previous.request_id),
                "a delta references the scope's last request"
            );
            let mut added = base[retained..].to_vec();
            added.extend(carried.iter().cloned());
            (
                json!({"kind":"delta","base_request_id":previous.request_id,
                    "retain_messages":retained,"prefix":&prefix,"suffix":&suffix,
                    "added_messages":added}),
                previous.segment_id.clone(),
            )
        } else {
            (
                json!({"kind":"full","json":body_json}),
                format!("segment-{sequence}"),
            )
        };
        for source in &captured {
            let mut row = source.clone();
            row["parent_tool_use_id"] = json!(parent);
            match source["type"].as_str().unwrap() {
                "system" | "result" => continue,
                "model_request" => {
                    row["parent_tool_use_id"] = Value::Null;
                    let evidence = &mut row["request"];
                    evidence["body"] = body.clone();
                    evidence["body_bytes"] = json!(body_json.len());
                    evidence["body_sha256"] = json!(hash(body_json.as_bytes()));
                    evidence["journal_id"] = json!("fixture");
                    evidence["sequence"] = json!(sequence);
                    evidence["request_id"] = json!(request_id);
                    evidence["owner"]["attempt_id"] = json!(attempt_id);
                    evidence["kv_scope"] = json!(scope);
                    evidence["segment_id"] = json!(segment_id);
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
                            let mut changed =
                                raw.replace(provider_id, &format!(r#""id":{}"#, json!(call_id)));
                            if call.is_none() {
                                let delivered = format!(
                                    r#","tool_calls":[{{"index":0,"id":{},"type":"function","function":{{"name":"audit_probe","arguments":"{{\"value\":1}}"}}}}]"#,
                                    json!(call_id)
                                );
                                assert_eq!(changed.matches(&delivered).count(), 1);
                                changed = changed
                                    .replace(&delivered, "")
                                    .replace(r#""finish_reason":"tool_calls""#, r#""finish_reason":"stop""#);
                            }
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
                    row["parent_tool_use_id"] = Value::Null;
                    let seed = &mut row["normalization_seed"];
                    seed["journal_id"] = json!("fixture");
                    seed["origin"] = origin.clone();
                    seed["request_id"] = json!(request_id);
                }
                "model_generation" => {
                    let evidence = &mut row["generation"];
                    let mut envelope = evidence["generation_json"]
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
                    if call.is_none() {
                        let mut parsed: Value = serde_json::from_str(&envelope).unwrap();
                        let observations = parsed["observations"].as_array_mut().unwrap();
                        observations[1]["tool_call_preparations"] = json!([]);
                        observations[2]["call_ids"] = json!([]);
                        let parts = observations[2]["response"]["candidates"][0]["content"]
                            ["parts"]
                            .as_array_mut()
                            .unwrap();
                        parts.retain(|part| part.get("functionCall").is_none());
                        envelope = parsed.to_string();
                    }
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
                "assistant" => {
                    // The settled turn's display; only its identity is the writer's.
                    if call.is_none() {
                        // Without its call the turn ends with its text message,
                        // which then carries the turn's usage.
                        match row["message"]["content"][0]["type"].as_str() {
                            Some("tool_use") => continue,
                            Some("text") => {
                                let last = captured
                                    .iter()
                                    .rfind(|row| row["type"] == "assistant")
                                    .unwrap();
                                row["message"]["usage"] = last["message"]["usage"].clone();
                            }
                            _ => {}
                        }
                    }
                    row["message"]["id"] = json!(format!("shown-{}", self.rows.len()));
                    for block in row["message"]["content"].as_array_mut().unwrap() {
                        if block["type"] == "tool_use" {
                            block["id"] = json!(call_id);
                        }
                    }
                }
                "stream_event" => {
                    // A subagent scope streams bare content blocks.
                    let group = matches!(
                        row["event"]["type"].as_str(),
                        Some("message_start" | "message_stop")
                    );
                    if (parent.is_some() && group) || (call.is_none() && !group) {
                        continue;
                    }
                    if row["event"]["type"] == "content_block_start" {
                        row["event"]["content_block"]["id"] = json!(call_id);
                    }
                }
                _ => panic!("unexpected captured fixture record"),
            }
            self.push(row);
        }
        self.physical += 1;
        self.requests.push((scope.into(), usage));
        self.last_request.insert(scope.into(), request_id.clone());
        self.chats.insert(
            scope.into(),
            Invocation {
                request_id,
                segment_id,
                messages,
                turn: match call {
                    Some(call_id) => json!({"role":"assistant","content":"before  after",
                        "tool_calls":[{"id":call_id,"type":"function",
                            "function":{"name":"audit_probe","arguments":"{\"value\":1}"}}]}),
                    None => json!({"role":"assistant","content":"before  after"}),
                }
                .to_string(),
            },
        );
    }

    /// One completed chat-tokenizer operation measuring `count` in `kv`.
    fn tokenizer(&mut self, kv: &str, role: &str, count: u64) -> String {
        let sequence = self.next_sequence();
        let operation_id = format!("token-{sequence}-{role}");
        let request_id = format!("tokenizer-{sequence}");
        let body = json!({"model":COMPACTION_MODEL,"messages":[],"add_generation_prompt":true})
            .to_string();
        self.push(json!({"type":"model_utility_request","utility_request":{
            "journal_id":"fixture","sequence":sequence,"request_id":request_id,
            "operation_id":operation_id,"kv_scope":kv,"kind":"tokenize_chat",
            "requested_model":COMPACTION_MODEL,"requested_input_count":null,
            "expected_max_model_len":TOKENIZER_WINDOW,
            "request_url":"http://fixture.invalid/tokenize","body_json":body,
            "body_bytes":body.len(),"body_sha256":hash(body.as_bytes())}}));
        let result = json!({"count":count,"max_model_len":TOKENIZER_WINDOW}).to_string();
        for (index, event) in [
            json!({"kind":"http","status":200,"content_type":"application/json"}),
            json!({"kind":"body","offset":0,"base64":STANDARD.encode(result.as_bytes())}),
            json!({"kind":"end","termination":"eof","body_bytes":result.len(),
                "body_sha256":hash(result.as_bytes()),"error":null}),
            json!({"kind":"outcome","status":"completed","error":null,"served_usage":null,
                "sdk_values_seen":1,"pipeline_outputs_delivered":0}),
            json!({"kind":"delivery","outputs_delivered":0}),
        ]
        .into_iter()
        .enumerate()
        {
            self.response(&request_id, index as u64 + 1, event);
        }
        self.push(json!({"type":"model_utility_completion","utility_completion":{
            "journal_id":"fixture","operation_id":operation_id,"kv_scope":kv,
            "kind":"tokenize_chat","requested_model":COMPACTION_MODEL,
            "requested_input_count":null,"expected_max_model_len":TOKENIZER_WINDOW,
            "request_ids":[request_id],
            "result":{"kind":"token_count","total_tokens":count,"max_model_len":TOKENIZER_WINDOW},
            "error":null}}));
        operation_id
    }

    /// The physical requests of one compaction draw: earlier retries deliver
    /// nothing; the final request streams exactly the draw's SDK values, and
    /// the client delivers the decoded observation its content projects from.
    /// A redraw's request is its draw's request with the refusal notice for
    /// the draw before it after the directive, as the client builds it.
    fn compaction_draw(
        &mut self,
        kv: &str,
        operation: &str,
        budget: u64,
        draw: &Value,
        redrawn: bool,
    ) {
        let requests = draw["physicalRequests"].as_u64().unwrap().max(1);
        for attempt in 1..=requests {
            let last = attempt == requests;
            let sequence = self.next_sequence();
            let id = format!("request-{sequence}");
            let mut messages = vec![json!({"role":"user","content":[{"type":"text","text":format!(
                "your answer may generate at most {budget} tokens, reasoning included. If all draws are refused, this conversation cannot continue."
            )}]})];
            if redrawn {
                messages.push(json!({"role":"user","content":[{"type":"text","text":
                    "Your previous answer to this request was refused because its snapshot renders to 38102 bytes, past the 32768-byte limit. Write the same state more briefly."}]}));
            }
            let body = json!({"kv_scope":kv,"model":COMPACTION_MODEL,"stream":true,
                "max_tokens":budget,"messages":messages})
            .to_string();
            self.push(json!({"type":"model_request","request":{
                "journal_id":"fixture","sequence":sequence,"request_id":id,"kv_scope":kv,
                "segment_id":format!("segment-{sequence}"),"prompt_id":"compaction",
                "owner":{"kind":"utility","operation_id":operation,"purpose":"compaction"},
                "decode_policy":decode_policy("stream",COMPACTION_MODEL),
                "body":{"kind":"full","json":body},"body_bytes":body.len(),
                "body_sha256":hash(body.as_bytes())}}));
            self.last_request.insert(kv.into(), id.clone());
            self.response(
                &id,
                1,
                json!({"kind":"http","status":200,"content_type":"text/event-stream"}),
            );
            let values: Vec<&str> = if last {
                draw["sdkValuesJson"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap())
                    .collect()
            } else {
                Vec::new()
            };
            let finished = last && !draw["finishReason"].is_null();
            let mut stream: String = values.iter().map(|value| format!("data: {value}\n\n")).collect();
            if finished {
                stream.push_str("data: [DONE]\n\n");
            }
            let mut next = 2;
            if !stream.is_empty() {
                self.response(
                    &id,
                    next,
                    json!({"kind":"body","offset":0,"base64":STANDARD.encode(stream.as_bytes())}),
                );
                next += 1;
            }
            self.response(
                &id,
                next,
                json!({"kind":"end","termination":"eof","body_bytes":stream.len(),
                    "body_sha256":hash(stream.as_bytes()),"error":null}),
            );
            next += 1;
            let delivered = last && draw_has_content(draw);
            if delivered {
                self.decoded(&id, next, &decoded_draw(draw));
                next += 2;
            }
            let usage = values
                .iter()
                .rev()
                .filter_map(|value| serde_json::from_str::<Value>(value).ok())
                .find_map(|value| value.get("usage").filter(|usage| usage.is_object()).cloned())
                .map_or(Value::Null, |usage| {
                    json!({"promptTokenCount":usage["prompt_tokens"],
                        "candidatesTokenCount":usage["completion_tokens"],
                        "totalTokenCount":usage["total_tokens"],
                        "cachedContentTokenCount":usage["prompt_tokens_details"]["cached_tokens"],
                        "thoughtsTokenCount":usage["completion_tokens_details"]["reasoning_tokens"]})
                });
            self.response(
                &id,
                next,
                json!({"kind":"outcome","status":if finished { "completed" } else { "failed" },
                    "error":if finished { Value::Null } else { json!("the draw ended before its finish") },
                    "served_usage":usage,"sdk_values_seen":values.len(),
                    "pipeline_outputs_delivered":u64::from(delivered)}),
            );
            self.response(
                &id,
                next + 1,
                json!({"kind":"delivery","outputs_delivered":u64::from(delivered)}),
            );
            self.requests.push((kv.into(), Some(usage)));
        }
    }

    /// Record the physical evidence of one compaction transition in the
    /// record's scope (preflight tokenizer measurements, every drawn
    /// candidate's requests, a measurement per measured candidate), then the
    /// record itself naming that evidence.
    fn compaction(&mut self, parent: Option<&str>, mut record: Value) {
        let kv = parent.unwrap_or("a").to_string();
        let original = record["data"]["originalTokenCount"].as_u64().unwrap();
        let budget = record["data"]["output"]["maxOutputTokens"]
            .as_u64()
            .unwrap_or(COMPACTION_BUDGET);
        let mut measurements = Vec::new();
        for (role, count) in [("original", original), ("summary_request", 20), ("prompt_only", 18)] {
            let operation = self.tokenizer(&kv, role, count);
            measurements.push(json!({"role":role,"operationId":operation}));
        }
        let transition = self.rows.len();
        let rejected = record["data"]["rejectedAttempts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut draws: Vec<(Value, String, Option<usize>)> = rejected
            .into_iter()
            .enumerate()
            .map(|(index, draw)| (draw, format!("compaction-{transition}-rejected-{index}"), Some(index)))
            .collect();
        if record["data"]["output"].is_object() {
            draws.push((
                record["data"]["output"].clone(),
                format!("compaction-{transition}-output"),
                None,
            ));
        }
        for (ordinal, (draw, operation, index)) in draws.into_iter().enumerate() {
            match index {
                Some(index) => record["data"]["rejectedAttempts"][index]["operationId"] = json!(operation),
                None => record["data"]["output"]["operationId"] = json!(operation),
            }
            self.compaction_draw(&kv, &operation, budget, &draw, ordinal > 0);
            if let Some(count) = draw["newTokenCount"].as_u64() {
                let measured = self.tokenizer(&kv, "candidate", count);
                measurements.push(json!({"role":"candidate","operationId":measured}));
            }
        }
        record["data"]["tokenMeasurements"] = json!(measurements);
        record["parent_tool_use_id"] = json!(parent);
        self.push(record);
    }

    /// A runtime answer. Its only producer is the root: a runtime operation
    /// receipt has no parent, so a child scope never carries one.
    fn presentation(&mut self) {
        let parent: Option<&str> = None;
        let text = "Runtime status.";
        let id = format!("local-operation-{}", self.rows.len());
        self.push(json!({"type":"system","subtype":"runtime_operation",
            "parent_tool_use_id":parent,"data":{"operation_id":id,
                "kind":"slash_message","output_sha256":hash(text.as_bytes()),
                "output_bytes":text.len()}}));
        self.push(json!({"type":"assistant","parent_tool_use_id":parent,
            "message":{"id":format!("notice-{}", self.rows.len()),
                "type":"message","role":"assistant","model":self.model,"stop_reason":null,
                "content":[{"type":"text","text":text}],"usage":null}}));
        self.last_model_text
            .insert(parent.map(str::to_string), text.into());
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
                "request_count":self.physical,"open_response_ids":[],"open_attempt_ids":[]});
        }
        if let Some((subtype, message)) = error {
            row["subtype"] = json!(subtype);
            row["is_error"] = json!(true);
            row["error"] = json!({"message":message});
            row.as_object_mut().unwrap().remove("result");
            // A run that ends without its declared deliverables names them.
            if subtype == MISSING_DELIVERABLES_SUBTYPE {
                row["missing_deliverables"] = json!(["report.md"]);
            }
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

const COMPACTION_MODEL: &str = "fixture-model";
const COMPACTION_BUDGET: u64 = 49152;
const TOKENIZER_WINDOW: u64 = 262_144;

fn draw_has_content(draw: &Value) -> bool {
    ["text", "reasoning"]
        .iter()
        .any(|key| draw[*key].as_str().is_some_and(|text| !text.is_empty()))
        || ["functionCalls", "incompleteToolCalls"]
            .iter()
            .any(|key| draw[*key].as_array().is_some_and(|items| !items.is_empty()))
        || !draw["finishReason"].is_null()
        || !draw["usage"].is_null()
}

/// The decoded observation a draw's recorded content projects from.
fn decoded_draw(draw: &Value) -> Value {
    let mut parts = Vec::new();
    if let Some(reasoning) = draw["reasoning"].as_str().filter(|text| !text.is_empty()) {
        parts.push(json!({"text":reasoning,"thought":true}));
    }
    if let Some(text) = draw["text"].as_str().filter(|text| !text.is_empty()) {
        parts.push(json!({"text":text}));
    }
    for call in draw["functionCalls"].as_array().into_iter().flatten() {
        parts.push(json!({"functionCall":call}));
    }
    let mut candidate = json!({"content":{"parts":parts,"role":"model"},"index":0});
    if !draw["finishReason"].is_null() {
        candidate["finishReason"] = draw["finishReason"].clone();
    }
    let mut response = json!({"candidates":[candidate]});
    if !draw["usage"].is_null() {
        response["usageMetadata"] = draw["usage"].clone();
    }
    json!({"response":response,"incomplete_tool_calls":draw["incompleteToolCalls"],
        "tool_call_preparations":[]})
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

/// A service-owned private event file holding exactly `bytes`.
fn owned_event_file(bytes: &[u8]) -> std::path::PathBuf {
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
    path
}

/// The native certification of one opened snapshot: descriptor framing and
/// the pure runtime contract, without the physical response replay that the
/// service image's pinned verifier adds afterwards.
fn snapshot_bytes(bytes: &[u8]) -> ServiceResult<EventSnapshot> {
    let path = owned_event_file(bytes);
    let result = open_event_prefix(&path).and_then(|prefix| {
        read_opened_event_snapshot(&path, prefix.expect("test event file exists"))
    });
    std::fs::remove_file(path).unwrap();
    result
}

/// The service's complete snapshot read, including the physical replay.
fn full_snapshot_bytes(bytes: &[u8]) -> ServiceResult<EventSnapshot> {
    let path = owned_event_file(bytes);
    let result =
        read_event_snapshot(&path).map(|snapshot| snapshot.expect("test event file exists"));
    std::fs::remove_file(path).unwrap();
    result
}
