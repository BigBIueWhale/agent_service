//! The displayed conversation binds to model input at the native layer: every
//! user row a scope shows is what the scope's next chat request carries, and a
//! settled turn is shown exactly as its generation projects it.
use super::*;

/// A tool call, its displayed result, then a final text answer whose request
/// carries that result.
fn two_turns(delta: bool) -> Trace {
    let mut trace = Trace::new();
    trace.chat(None, "first");
    trace.tool_result(None, "first", "done");
    trace.answer(None, delta);
    trace.terminal(None, 2, None);
    trace
}

fn position(trace: &Trace, found: impl Fn(&Value) -> bool) -> usize {
    trace.rows.iter().position(found).unwrap()
}

fn user_row(trace: &Trace) -> usize {
    position(trace, |row| row["type"] == "user")
}

fn last_request(trace: &Trace) -> usize {
    trace
        .rows
        .iter()
        .rposition(|row| row["type"] == "model_request")
        .unwrap()
}

#[test]
fn a_displayed_tool_result_reaches_the_next_request_in_full_and_delta_bodies() {
    for delta in [false, true] {
        let trace = two_turns(delta);
        let request = &trace.rows[last_request(&trace)]["request"]["body"];
        assert_eq!(request["kind"], if delta { "delta" } else { "full" });
        let result = trace.certify();
        assert_eq!(result.response, "before  after");
        assert_eq!(result.num_turns, 2);
        assert_eq!(result.usage.requests, 2);
    }
}

#[test]
fn consecutive_delta_turns_carry_each_displayed_result() {
    let mut trace = Trace::new();
    trace.chat(None, "first");
    trace.tool_result(None, "first", "done");
    trace.chat_delta(None, "second");
    trace.tool_result(None, "second", "done again");
    trace.answer(None, true);
    trace.terminal(None, 3, None);
    let deltas = trace
        .rows
        .iter()
        .filter(|row| row["request"]["body"]["kind"] == "delta")
        .count();
    assert_eq!(deltas, 2);
    assert_eq!(trace.certify().usage.requests, 3);
}

#[test]
fn a_displayed_notice_reaches_the_next_request() {
    let mut trace = Trace::new();
    trace.notice(None, "A notice before the first turn.");
    trace.chat(None, "first");
    trace.tool_result(None, "first", "done");
    trace.notice(None, "A notice after the tool result.");
    trace.answer(None, true);
    trace.terminal(None, 2, None);
    trace.certify();
}

#[test]
fn a_dropped_tool_result_leaves_the_next_request_without_its_displayed_result() {
    for delta in [false, true] {
        let mut trace = two_turns(delta);
        trace.rows.remove(user_row(&trace));
        assert_refused_at(
            &trace,
            "sends the next request in the main session without the displayed result of call \"first\"",
        );
    }
}

#[test]
fn a_forged_tool_result_differs_from_what_the_next_request_carries() {
    for delta in [false, true] {
        let mut trace = two_turns(delta);
        let row = user_row(&trace);
        trace.rows[row]["message"]["content"][0]["content"] = json!("FORGED done");
        assert_refused_at(
            &trace,
            "does not carry displayed input tool result \"first\"",
        );
    }
}

#[test]
fn a_rehashed_delta_whose_tool_message_contradicts_the_display_is_refused() {
    let mut trace = two_turns(true);
    let request = last_request(&trace);
    let forged = json!({"role":"tool","tool_call_id":"first","content":"FORGED"}).to_string();
    let carried = json!({"role":"tool","tool_call_id":"first","content":"done"}).to_string();
    let messages: Vec<String> = trace.chats["a"]
        .messages
        .iter()
        .map(|message| {
            if *message == carried {
                forged.clone()
            } else {
                message.clone()
            }
        })
        .collect();
    assert!(messages.contains(&forged));
    let body = &mut trace.rows[request]["request"]["body"];
    for added in body["added_messages"].as_array_mut().unwrap() {
        if *added == json!(carried) {
            *added = json!(forged);
        }
    }
    let full = format!(
        "{}{}{}",
        body["prefix"].as_str().unwrap(),
        messages.join(","),
        body["suffix"].as_str().unwrap()
    );
    // Consistently rehashed: the physical request itself is well formed.
    trace.rows[request]["request"]["body_bytes"] = json!(full.len());
    trace.rows[request]["request"]["body_sha256"] = json!(hash(full.as_bytes()));
    assert_refused_at(
        &trace,
        "does not carry displayed input tool result \"first\"",
    );
}

#[test]
fn an_injected_notice_the_model_never_received_is_refused() {
    let mut trace = two_turns(true);
    let notice = json!({"type":"user","uuid":"injected-notice","session_id":"a",
        "parent_tool_use_id":null,"message":{"role":"user",
            "content":[{"type":"text","text":"a notice the model never saw"}]}});
    trace.rows.insert(user_row(&trace) + 1, notice);
    assert_refused_at(&trace, "does not carry displayed input notice");
}

#[test]
fn a_user_row_between_a_settled_turn_and_its_display_is_refused() {
    let mut trace = two_turns(false);
    let user = user_row(&trace);
    let shown = trace.rows[..user]
        .iter()
        .rposition(|row| row["type"] == "assistant")
        .unwrap();
    trace.rows.swap(shown, user);
    let line = shown + 1;
    assert_refused_at(
        &trace,
        &format!("line {line} precedes the assistant messages its settled turn in the main session shows"),
    );
}

#[test]
fn user_content_is_a_closed_array_of_text_or_tool_results() {
    let complete = two_turns(false);
    complete.certify();
    let row = user_row(&complete);
    let mut cases: Vec<(&str, Trace)> = Vec::new();
    let mut unknown_field = complete.clone();
    unknown_field.rows[row]["message"]["extra"] = json!(1);
    cases.push(("unknown field in user.message", unknown_field));
    let mut unknown_block = complete.clone();
    unknown_block.rows[row]["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"image","source":"x"}));
    cases.push(("unknown block type", unknown_block));
    let mut not_array = complete.clone();
    not_array.rows[row]["message"]["content"] = json!("text");
    cases.push(("non-array content", not_array));
    let mut unflagged = complete.clone();
    unflagged.rows[row]["message"]["content"][0]
        .as_object_mut()
        .unwrap()
        .remove("is_error");
    cases.push(("tool result without is_error", unflagged));
    let mut structured = complete.clone();
    structured.rows[row]["message"]["content"][0]["content"] =
        json!([{"type":"text","text":"done"}]);
    cases.push(("tool result with block content", structured));
    for (label, trace) in cases {
        let error = trace.snapshot().certified.unwrap_err().to_string();
        assert!(
            error.contains(&format!("line {} violates stream contract", row + 1)),
            "{label}: {error}"
        );
    }
    let mut mixed = complete;
    mixed.rows[row]["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"text","text":"done"}));
    assert_refused_at(&mixed, "mixes tool results with other user content");
}

#[test]
fn every_record_kind_is_closed_at_its_top_level() {
    let mut complete = Trace::new();
    complete.utility("a", ordinary_usage());
    complete.chat(None, "first");
    complete.tool_result(None, "first", "done");
    complete.answer(None, true);
    complete.terminal(None, 2, None);
    complete.certify();
    let mut kinds = std::collections::BTreeSet::new();
    for index in 0..complete.rows.len() {
        let mut trace = complete.clone();
        trace.rows[index]["extra"] = json!(true);
        kinds.insert(trace.rows[index]["type"].as_str().unwrap().to_string());
        let error = trace.snapshot().certified.unwrap_err().to_string();
        assert!(
            error.contains(&format!("line {} violates stream contract", index + 1)),
            "{}: {error}",
            trace.rows[index]
        );
    }
    for kind in [
        "system",
        "model_request",
        "model_response",
        "model_normalization_seed",
        "model_generation",
        "model_attempt_completion",
        "assistant",
        "user",
        "stream_event",
        "result",
    ] {
        assert!(kinds.contains(kind), "{kind} was not exercised");
    }
}

#[test]
fn a_subagent_scope_streams_bare_content_blocks() {
    let mut trace = Trace::new();
    trace.chat(None, "child");
    trace.chat(Some("child"), "grandchild");
    trace.terminal(Some("child"), 1, None);
    trace.terminal(None, 1, None);
    assert!(trace
        .rows
        .iter()
        .filter(|row| row["type"] == "stream_event" && row["parent_tool_use_id"] == "child")
        .all(|row| row["event"]["type"]
            .as_str()
            .is_some_and(|kind| kind.starts_with("content_block_"))));
    let result = trace.certify();
    assert_eq!(result.scopes.len(), 1);

    let first = position(&trace, |row| {
        row["type"] == "stream_event" && row["parent_tool_use_id"] == "child"
    });
    let mut grouped = trace;
    let mut start = grouped.rows[first].clone();
    start["uuid"] = json!("child-message-start");
    start["event"] = json!({"type":"message_start","message":{"id":"child-message",
        "role":"assistant","model":"test","content":[]}});
    grouped.rows.insert(first, start);
    assert_refused_at(&grouped, "a subagent scope has no message group");
}

#[test]
fn the_display_of_a_settled_turn_is_exactly_its_generation() {
    let complete = two_turns(false);
    complete.certify();
    let shown: Vec<usize> = complete
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row["type"] == "assistant")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(shown.len(), 5, "three messages for the call turn, two for the answer");
    let tool_use = shown[2];
    let cases: Vec<(&str, usize, Box<dyn Fn(&mut Value)>)> = vec![
        ("content", shown[4], Box::new(|message| message["content"][0]["text"] = json!("FORGED"))),
        ("usage", shown[4], Box::new(|message| {
            let output = message["usage"]["output_tokens"].as_u64().unwrap();
            message["usage"]["output_tokens"] = json!(output + 1);
        })),
        ("stop reason", tool_use, Box::new(|message| message["stop_reason"] = Value::Null)),
        ("content", tool_use, Box::new(|message| {
            message["content"][0]["input"] = json!({"file_path":"/etc/passwd","offset":0});
        })),
        ("model", shown[0], Box::new(|message| message["model"] = json!("another-model"))),
        ("usage", shown[0], Box::new(|message| {
            message["usage"] = shown_usage(12, 7, 4, 2, 19);
        })),
    ];
    for (what, index, change) in cases {
        let mut trace = complete.clone();
        change(&mut trace.rows[index]["message"]);
        assert_refused_at(
            &trace,
            &format!("line {} shows a settled turn with {}", index + 1, if what == "stop reason" || what == "model" { format!("a {what}") } else { what.to_string() }),
        );
    }
    let mut dropped = complete.clone();
    dropped.rows.remove(tool_use);
    assert_refused_at(
        &dropped,
        &format!("line {} precedes the assistant messages", tool_use + 1),
    );
    let mut extra = complete.clone();
    let mut copy = extra.rows[shown[4]].clone();
    copy["uuid"] = json!("extra-assistant");
    copy["message"]["id"] = json!("extra-assistant");
    extra.rows.insert(shown[4] + 1, copy);
    assert_refused_at(
        &extra,
        &format!("line {} claims runtime assistant output after a chat attempt", shown[4] + 2),
    );
}

fn shown_usage(input: u64, output: u64, cached: u64, reasoning: u64, total: u64) -> Value {
    json!({"input_tokens":input,"output_tokens":output,"cache_read_input_tokens":cached,
        "reasoning_output_tokens":reasoning,"total_tokens":total})
}
