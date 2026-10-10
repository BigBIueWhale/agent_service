use super::*;

/// The provider's final include_usage chunk reporting `usage`.
fn usage_chunk(usage: &Value) -> String {
    json!({"choices":[],"usage":wire_usage(usage)}).to_string()
}

/// A truncated candidate: the provider completed the draw at its ceiling while
/// the snapshot call was still being written.
fn output() -> Value {
    let usage = served(233926, 49152, 49152, 100);
    json!({"maxOutputTokens":COMPACTION_BUDGET,"physicalRequests":2,
        "operationId":"named-when-recorded","functionCalls":[],
        "text":"  partial snapshot","reasoning":"Observed reasoning.  ",
        "sdkValuesJson":["{\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"state_snapshot\",\"arguments\":\"{\\\"intent\\\":\"}}]}}]}", usage_chunk(&usage)],
        "newTokenCount":null,"snapshotTokens":null,"snapshotCountOperationId":null,
        "incompleteToolCalls":[{"name":"state_snapshot","arguments":"{\"intent\": \"Summarise the corpus \\u2014 cut"}],
        "finishReason":"MAX_TOKENS","usage":usage})
}

fn rejected() -> Value {
    let usage = served(233926, 40, 40, 100);
    json!({"status":"COMPRESSION_FAILED_EMPTY_SUMMARY","physicalRequests":1,
        "operationId":"named-when-recorded","functionCalls":[],
        "text":"","reasoning":"Reasoning that produced nothing.",
        "sdkValuesJson":["{\"choices\":[{\"delta\":{\"reasoning_content\":\"Reasoning that produced nothing.\"}}]}", usage_chunk(&usage)],
        "newTokenCount":null,"snapshotTokens":null,"snapshotCountOperationId":null,"incompleteToolCalls":[],
        "finishReason":"STOP","usage":usage})
}

fn record(output: Value, rejected: Value) -> Value {
    json!({"type":"system","subtype":"compaction","parent_tool_use_id":null,
        "data":{"status":"COMPRESSION_FAILED_OUTPUT_TRUNCATED","succeeded":false,
            "originalTokenCount":233926,"newTokenCount":233926,"triggerReason":"token_limit",
            "output":output,"postCompactionHistory":null,"rejectedAttempts":rejected}})
}

fn snapshot_sections() -> Value {
    json!({
        "primary_request_and_intent":"None", "key_technical_concepts":"None",
        "files_and_code_sections":"None", "errors_and_fixes":"None",
        "problem_solving":"None", "pending_tasks":"None", "current_work":"None",
        "next_step":"None"
    })
}

/// A committed replacement: the accepted draw is one complete state_snapshot
/// call whose rendered size and measured count the transition reports.
fn committed(history: Value) -> Value {
    committed_with(history, snapshot_sections())
}

/// A committed replacement accepted from a draw whose call declared `sections`.
fn committed_with(history: Value, sections: Value) -> Value {
    let usage = served(233926, 4, 0, 100);
    let provider = json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"snapshot-call",
        "type":"function","function":{"name":"state_snapshot","arguments":sections.to_string()}}]},
        "finish_reason":"tool_calls"}]})
    .to_string();
    let draw = json!({"maxOutputTokens":COMPACTION_BUDGET,"physicalRequests":1,
        "operationId":"named-when-recorded",
        "functionCalls":[{"id":"snapshot-call","name":"state_snapshot","args":sections}],
        "text":"","reasoning":"","sdkValuesJson":[provider, usage_chunk(&usage)],
        "newTokenCount":12,"snapshotTokens":131,"snapshotCountOperationId":"named-when-recorded","incompleteToolCalls":[],
        "finishReason":"STOP","usage":usage});
    let mut record = record(draw, json!([]));
    record["data"]["status"] = json!("COMPRESSED");
    record["data"]["succeeded"] = json!(true);
    record["data"]["newTokenCount"] = json!(12);
    record["data"]["postCompactionHistory"] = history;
    record
}

/// Field-validation fixtures carry explicit physical draws, preflight and
/// candidate tokenizer measurements recorded in the compaction's own scope.
/// Their synthetic requests are not evidence for the production compactor's
/// ownership or for resumed history; those paths need their own tests.
fn trace_with(record: Value, child: bool) -> Trace {
    let mut trace = Trace::new();
    if child {
        trace.chat(None, "child");
    }
    trace.compaction(child.then_some("child"), record);
    trace.terminal(None, u64::from(child), None);
    trace
}

fn data(trace: &mut Trace) -> &mut Value {
    &mut trace
        .rows
        .iter_mut()
        .find(|row| row["subtype"] == "compaction")
        .unwrap()["data"]
}

fn assert_compaction_refusal(trace: &Trace, expected: &str) {
    let snapshot = trace.snapshot();
    assert_eq!(observed_usage(&snapshot), trace.summary(None));
    let error = snapshot.certified.unwrap_err().to_string();
    let line = trace
        .rows
        .iter()
        .position(|row| row["subtype"] == "compaction")
        .unwrap()
        + 1;
    assert!(error.contains(&format!("line {line} ")), "{error}");
    assert!(error.contains(expected), "expected {expected:?}: {error}");
}

/// The schema's refusal of the whole record variant.
const SCHEMA: &str = "schema rule /oneOf";

#[test]
fn compaction_preserves_served_interrupted_and_unattempted_observations() {
    // The stream ended before the provider's usage report: a request that
    // broke, which the transition ended on.
    for (observation, status) in [
        (output(), "COMPRESSION_FAILED_OUTPUT_TRUNCATED"),
        (interrupted(), "COMPRESSION_FAILED_TRANSPORT_ERROR"),
        (interrupted(), "COMPRESSION_FAILED_PROTOCOL_ERROR"),
        (Value::Null, "COMPRESSION_FAILED_OUTPUT_TRUNCATED"),
    ] {
        let mut record = record(observation, json!([]));
        record["data"]["status"] = json!(status);
        let trace = trace_with(record, false);
        trace.certify();
        assert_eq!(observed_usage(&trace.snapshot()), trace.summary(None));
    }
}

/// The truncated candidate, cut off before its finish and its usage report:
/// a request whose transport failed after it had streamed part of a draw.
fn interrupted() -> Value {
    let mut interrupted = output();
    interrupted["usage"] = Value::Null;
    interrupted["finishReason"] = Value::Null;
    interrupted["sdkValuesJson"].as_array_mut().unwrap().pop();
    interrupted
}

#[test]
fn a_draw_whose_request_failed_in_transport_is_kept_and_issued_again() {
    // The request broke before the draw had an answer: kept with what it had
    // streamed, named by the fault, and followed by the same request issued
    // again, whose draw settles the transition.
    let history = json!([{"role":"user","parts":[{"text":"accepted snapshot"}]}]);
    let mut faulted = interrupted();
    faulted.as_object_mut().unwrap().remove("maxOutputTokens");
    faulted["status"] = json!("COMPRESSION_FAILED_TRANSPORT_ERROR");
    let mut record = committed(history);
    record["data"]["rejectedAttempts"] = json!([faulted.clone()]);
    trace_with(record.clone(), false).certify();
    // An answer refused by a rule and a request that broke are different
    // things, and each is held to the physical request it names: a fault its
    // request completed, and a refusal its request never finished, are both
    // refused.
    let mut completed = rejected();
    completed["status"] = json!("COMPRESSION_FAILED_TRANSPORT_ERROR");
    let mut forged = record.clone();
    forged["data"]["rejectedAttempts"] = json!([completed]);
    assert_physical_refusal(
        &trace_with(forged, false),
        "claims a transport fault its physical request did not have",
    );
    let mut unfinished = faulted;
    unfinished["status"] = json!("COMPRESSION_FAILED_OUTPUT_TRUNCATED");
    let mut forged = record;
    forged["data"]["rejectedAttempts"] = json!([unfinished]);
    assert_physical_refusal(
        &trace_with(forged, false),
        "claims an answer its physical request did not complete and deliver",
    );
}

/// A refusal of what a compaction draw claims of its physical request, which
/// the request evidence names rather than the record's line.
fn assert_physical_refusal(trace: &Trace, expected: &str) {
    let snapshot = trace.snapshot();
    assert_eq!(observed_usage(&snapshot), trace.summary(None));
    let error = snapshot.certified.unwrap_err().to_string();
    assert!(error.contains(expected), "expected {expected:?}: {error}");
}

#[test]
fn unknown_retained_counts_require_unsuccessful_compaction() {
    for child in [false, true] {
        let mut trace = trace_with(record(Value::Null, json!([])), child);
        for status in [
            "COMPRESSION_FAILED_HISTORY_CHANGED",
            "COMPRESSION_FAILED_PROTOCOL_ERROR",
        ] {
            data(&mut trace)["status"] = json!(status);
            data(&mut trace)["newTokenCount"] = Value::Null;
            trace.certify();
        }
        // Use a complete drawn candidate before testing a successful history.
        let mut success = trace_with(
            committed(json!([{"role":"user","parts":[{"text":"accepted snapshot"}]}])),
            child,
        );
        success.certify();
        data(&mut success)["newTokenCount"] = Value::Null;
        assert_compaction_refusal(&success, "measured token count");
        data(&mut trace)
            .as_object_mut()
            .unwrap()
            .remove("newTokenCount");
        assert_compaction_refusal(&trace, "newTokenCount");
    }
}

#[test]
fn compaction_draws_require_content_measurements_and_decodable_provider_values() {
    let complete = trace_with(record(output(), json!([])), false);
    complete.certify();
    // Every draw field is required by the shared definition.
    for field in [
        "sdkValuesJson",
        "text",
        "newTokenCount",
        "snapshotTokens",
        "snapshotCountOperationId",
        "usage",
        "physicalRequests",
        "finishReason",
        "operationId",
        "functionCalls",
    ] {
        let mut trace = complete.clone();
        data(&mut trace)["output"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_compaction_refusal(&trace, SCHEMA);
    }
    for (responses, expected) in [
        (json!([]), SCHEMA),
        (json!(["not JSON"]), "undecodable SDK value JSON"),
    ] {
        let mut trace = complete.clone();
        data(&mut trace)["output"]["sdkValuesJson"] = responses;
        assert_compaction_refusal(&trace, expected);
    }
    let mut trace = complete;
    data(&mut trace)["output"]["unknown_evidence"] = json!(true);
    assert_compaction_refusal(&trace, "violates stream contract");
}

#[test]
fn compaction_retains_a_non_json_sdk_value_when_conversion_fails() {
    // The stream yielded one scalar SDK value the converter could not use:
    // the draw keeps it verbatim and claims no decoded output.
    let value = "provider spoke plain text";
    let mut draw = output();
    draw["physicalRequests"] = json!(1);
    draw["sdkValuesJson"] = json!([serde_json::to_string(value).unwrap()]);
    draw["usage"] = Value::Null;
    draw["text"] = json!("");
    draw["reasoning"] = json!("");
    draw["incompleteToolCalls"] = json!([]);
    draw["finishReason"] = Value::Null;
    // A draw the converter could not read ended the transition on a fault
    // outside the model's answer.
    let mut record = record(draw, json!([]));
    record["data"]["status"] = json!("COMPRESSION_FAILED_PROTOCOL_ERROR");
    let trace = trace_with(record, false);
    let outcome = trace
        .rows
        .iter()
        .rfind(|row| row["response"]["event"]["kind"] == "outcome")
        .unwrap();
    assert_eq!(outcome["response"]["event"]["status"], "failed");
    assert_eq!(outcome["response"]["event"]["sdk_values_seen"], 1);
    assert_eq!(outcome["response"]["event"]["pipeline_outputs_delivered"], 0);
    trace.certify();
}

#[test]
fn compaction_history_is_required_exactly_when_committed() {
    trace_with(record(output(), json!([])), false).certify();
    let mut trace = trace_with(committed(Value::Null), false);
    assert_compaction_refusal(&trace, SCHEMA);
    data(&mut trace)["postCompactionHistory"] = json!([
        {"role":"user","parts":[{"text":"<all_user_messages>\n"},
            {"text":"Original user text, verbatim.\n"},{"text":"</all_user_messages>"}]},
        {"role":"model","parts":[{"functionCall":{"id":"carried","name":"read_file","args":{"path":"a"}}}]}
    ]);
    trace.certify();
    data(&mut trace)["succeeded"] = json!(false);
    data(&mut trace)["status"] = json!("COMPRESSION_FAILED_HISTORY_CHANGED");
    assert_compaction_refusal(&trace, SCHEMA);
}

#[test]
fn every_refused_candidate_remains_recorded_beside_the_final_draw() {
    for final_draw in [output(), Value::Null] {
        trace_with(record(final_draw, json!([rejected(), rejected()])), false).certify();
    }
}

#[test]
fn refused_candidates_require_their_rule_attempts_and_stopped_calls() {
    let complete = trace_with(record(output(), json!([rejected()])), false);
    complete.certify();
    for (field, value, expected) in [
        ("status", Value::Null, "names neither a rule its answer failed nor a transport fault"),
        ("physicalRequests", json!(0), SCHEMA),
        ("incompleteToolCalls", Value::Null, SCHEMA),
        ("usage", served(233926, 49153, 40, 100), "does not nest"),
    ] {
        let mut trace = complete.clone();
        data(&mut trace)["rejectedAttempts"][0][field] = value;
        assert_compaction_refusal(&trace, expected);
    }
    for field in ["status", "incompleteToolCalls"] {
        let mut trace = complete.clone();
        data(&mut trace)["rejectedAttempts"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_compaction_refusal(&trace, SCHEMA);
    }
    for (bad, expected) in [
        (json!([7]), "rejected attempt 0 is not an object"),
        (json!({}), "rejectedAttempts is not an array"),
    ] {
        let mut trace = complete.clone();
        data(&mut trace)["rejectedAttempts"] = bad;
        assert_compaction_refusal(&trace, expected);
    }
    let mut trace = complete;
    data(&mut trace)
        .as_object_mut()
        .unwrap()
        .remove("rejectedAttempts");
    assert_compaction_refusal(&trace, "rejectedAttempts");
}

#[test]
fn compaction_output_refuses_malformed_fields_and_inconsistent_usage() {
    let complete = trace_with(record(output(), json!([])), false);
    complete.certify();
    for (field, value, expected) in [
        ("reasoning", Value::Null, SCHEMA),
        ("text", Value::Null, SCHEMA),
        ("physicalRequests", json!(0), SCHEMA),
        ("maxOutputTokens", json!(0), SCHEMA),
        ("usage", json!(7), SCHEMA),
        ("incompleteToolCalls", Value::Null, SCHEMA),
        (
            "incompleteToolCalls",
            json!([{"name":"","arguments":""}]),
            "stopped call",
        ),
        (
            "incompleteToolCalls",
            json!([{"name":"state_snapshot"}]),
            "stopped call",
        ),
        (
            "incompleteToolCalls",
            json!([{"name":null,"arguments":{"intent":"x"}}]),
            "stopped call",
        ),
        (
            "incompleteToolCalls",
            json!([{"name":null,"arguments":"","input":{}}]),
            "stopped call",
        ),
    ] {
        let mut trace = complete.clone();
        data(&mut trace)["output"][field] = value;
        assert_compaction_refusal(&trace, expected);
    }
    for (field, value) in [
        ("thoughtsTokenCount", 49153),
        ("cachedContentTokenCount", 233927),
        ("totalTokenCount", 42),
    ] {
        let mut trace = complete.clone();
        data(&mut trace)["output"]["usage"][field] = json!(value);
        assert_compaction_refusal(&trace, "does not nest");
    }
    let mut trace = complete;
    data(&mut trace)["output"]["usage"]
        .as_object_mut()
        .unwrap()
        .remove("promptTokenCount");
    assert_compaction_refusal(&trace, SCHEMA);
}

#[test]
fn compaction_counts_refuse_unsafe_integers_in_root_and_child_scopes() {
    let unsafe_count = runtime_contract::SAFE_INTEGER + 1;
    for child in [false, true] {
        let complete = trace_with(record(output(), json!([])), child);
        complete.certify();
        for field in [
            "originalTokenCount",
            "newTokenCount",
            "maxOutputTokens",
            "physicalRequests",
            "promptTokenCount",
            "candidatesTokenCount",
            "cachedContentTokenCount",
            "thoughtsTokenCount",
            "totalTokenCount",
        ] {
            let mut trace = complete.clone();
            let data = data(&mut trace);
            if ["originalTokenCount", "newTokenCount"].contains(&field) {
                data[field] = json!(unsafe_count);
            } else if ["maxOutputTokens", "physicalRequests"].contains(&field) {
                data["output"][field] = json!(unsafe_count);
            } else {
                let output = &mut data["output"];
                match field {
                    "promptTokenCount" | "cachedContentTokenCount" => {
                        output["usage"]["promptTokenCount"] = json!(unsafe_count);
                        output["usage"][field] = json!(unsafe_count);
                        output["usage"]["totalTokenCount"] = json!(unsafe_count + 49152);
                    }
                    "candidatesTokenCount" | "thoughtsTokenCount" => {
                        output["usage"]["candidatesTokenCount"] = json!(unsafe_count);
                        output["usage"][field] = json!(unsafe_count);
                        output["usage"]["totalTokenCount"] = json!(unsafe_count + 233926);
                        output["maxOutputTokens"] = json!(unsafe_count);
                    }
                    "totalTokenCount" => {
                        output["usage"] = json!({"promptTokenCount":unsafe_count - 1,
                        "candidatesTokenCount":1,"cachedContentTokenCount":100,"thoughtsTokenCount":1,
                        "totalTokenCount":unsafe_count})
                    }
                    _ => unreachable!(),
                }
            }
            let snapshot = trace.snapshot();
            assert_eq!(observed_usage(&snapshot), complete.summary(None));
            assert!(snapshot.certified.is_err(), "{field}, child={child}");
        }
    }
}

#[test]
fn compaction_measurement_and_budget_counts_accept_exact_safe_boundaries() {
    // Each count is bound to its physical source: the original and candidate
    // counts to their tokenizer responses and the ceiling to the request's
    // own output ceiling, so the boundary is recorded there as well.
    let max = runtime_contract::SAFE_INTEGER;
    for child in [false, true] {
        let mut draw = output();
        draw["maxOutputTokens"] = json!(max);
        draw["newTokenCount"] = json!(max);
        let mut boundary = record(draw, json!([]));
        boundary["data"]["originalTokenCount"] = json!(max);
        boundary["data"]["newTokenCount"] = json!(max);
        trace_with(boundary, child).certify();
    }
    let usage = served(max - 1, 1, 1, max - 1);
    let mut draw = output();
    draw["physicalRequests"] = json!(1);
    draw["maxOutputTokens"] = json!(max);
    draw["sdkValuesJson"][1] = json!(usage_chunk(&usage));
    draw["usage"] = usage;
    trace_with(record(draw, json!([])), false).certify();
}

/// A token count is a function of the body counted, so a client that counts a
/// body once cites the operation that already counted it, whenever it ran:
/// the turn's own count as the original, the earlier count of the prompt the
/// turn was issued against, and one operation for every compaction that
/// counted the same body. The draws still follow every count they stand on.
#[test]
fn a_measurement_cites_the_operation_that_counted_its_body_whenever_it_ran() {
    let mut trace = Trace::new();
    let first = record(output(), json!([]));
    let original = first["data"]["originalTokenCount"].as_u64().unwrap();
    let prompt_only = trace.tokenizer("a", "prompt_only", 18);
    let turn = trace.tokenizer("a", "original", original);
    let cited = [("original", turn.as_str()), ("prompt_only", prompt_only.as_str())];
    trace.compaction_citing(None, first, &cited);
    trace.compaction_citing(None, record(output(), json!([])), &cited);
    trace.terminal(None, 0, None);
    trace.certify();
    assert_eq!(
        trace
            .rows
            .iter()
            .filter(|row| row["subtype"] == "compaction")
            .map(|row| row["data"]["tokenMeasurements"][0]["operationId"].clone())
            .collect::<Vec<_>>(),
        vec![json!(turn), json!(turn)]
    );
}

/// A cited count must be one this scope's tokenizer completed.
#[test]
fn a_measurement_cannot_cite_another_scopes_count() {
    let mut trace = Trace::new();
    trace.chat(None, "child");
    let first = record(output(), json!([]));
    let original = first["data"]["originalTokenCount"].as_u64().unwrap();
    let elsewhere = trace.tokenizer("child", "original", original);
    trace.compaction_citing(None, first, &[("original", elsewhere.as_str())]);
    trace.terminal(None, 1, None);
    assert_refused_at(
        &trace,
        "compaction measurement has no completed chat tokenizer operation in its scope",
    );
}

/// The generation the last chat in `trace` accepted.
fn last_generation(trace: &Trace) -> String {
    trace
        .rows
        .iter()
        .rfind(|row| row["type"] == "model_attempt_completion")
        .unwrap()["completion"]["generation_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A compaction that failed replaced nothing, so the turns its conversation
/// accepted are owed by every request after it, as they were before; one
/// that committed left the snapshot its accepted draw declared and the turn
/// it carried, and those are what every later request owes.
#[test]
fn a_failed_compaction_keeps_what_its_conversation_owes_and_a_committed_one_resets_it() {
    let history = json!([{"role":"user","parts":[{"text":"retained input"}]}]);
    // The first turn, a compaction that failed below the admission limit,
    // the turn issued on the history as it stands, then a compaction that
    // committed, and the next turn on what it left.
    let build = |keep_first: bool, compacted: bool, carry_second: bool| {
        let mut trace = Trace::new();
        trace.chat(None, "first");
        trace.tool_result(None, "first", "done");
        trace.compaction(None, record(output(), json!([])));
        if !keep_first {
            let dropped = json!({"role":"user","content":"continue"}).to_string();
            let invocation = trace.chats.get_mut("a").unwrap();
            invocation.turn_authors = harness_authors(&dropped);
            invocation.turn = dropped;
        }
        trace.chat(None, "second");
        trace.tool_result(None, "second", "done");
        trace.compaction(None, committed(history.clone()));
        if compacted {
            trace.compacted("a");
        }
        if !carry_second {
            let dropped = json!({"role":"user","content":"continue"}).to_string();
            let invocation = trace.chats.get_mut("a").unwrap();
            invocation.turn_authors = harness_authors(&dropped);
            invocation.turn = dropped;
        }
        trace.answer(None, false);
        trace.terminal(None, 3, None);
        trace
    };
    build(true, true, true).certify();
    // The turn the failed compaction left in place, dropped from the next.
    let mut first = Trace::new();
    first.chat(None, "first");
    let generation = last_generation(&first);
    assert_refused_at(&build(false, true, true), &format!("does not carry generation {generation:?}'s turn"));
    // After the commit, the history as it stood before it, or the snapshot
    // without the turn it carried.
    assert_refused_at(&build(true, false, true), "does not carry the snapshot compaction draw");
    let trace = build(true, true, true);
    let second = trace
        .rows
        .iter()
        .filter(|row| row["type"] == "model_attempt_completion")
        .nth(1)
        .unwrap()["completion"]["generation_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_refused_at(&build(true, true, false), &format!("does not carry generation {second:?}'s turn"));
}

/// A compaction commits the snapshot its accepted draw declared, and that is
/// what every later request of its conversation carries. A draw that
/// declared no complete snapshot commits nothing a request could carry.
#[test]
fn a_committed_compaction_was_accepted_from_a_draw_that_declared_a_snapshot() {
    let history = json!([{"role":"user","parts":[{"text":"retained input"}]}]);
    let mut sections = snapshot_sections();
    sections.as_object_mut().unwrap().remove("next_step");
    let mut record = committed_with(history.clone(), sections);
    record["data"]["output"]["snapshotTokens"] = Value::Null;
    record["data"]["output"]["snapshotCountOperationId"] = Value::Null;
    assert_compaction_refusal(
        &trace_with(record, false),
        "whose committed replacement was accepted from a draw that declared no complete snapshot",
    );
    trace_with(committed(history), false).certify();
}
