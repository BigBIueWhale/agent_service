use super::*;

fn output() -> Value {
    json!({"maxOutputTokens":49152,"requestAttempts":2,
        "text":"  partial snapshot","reasoning":"Observed reasoning.  ",
        "rawResponses":["{\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"state_snapshot\",\"arguments\":\"{\\\"intent\\\":\"}}]}}]}"],
        "newTokenCount":null,"snapshotBytes":null,
        "incompleteToolCalls":[{"name":"state_snapshot","arguments":"{\"intent\": \"Summarise the corpus \\u2014 cut"}],
        "finishReason":"MAX_TOKENS","usage":served(233926, 49152, 49152, 100)})
}

fn rejected() -> Value {
    json!({"status":"COMPRESSION_FAILED_EMPTY_SUMMARY","requestAttempts":1,
        "text":"","reasoning":"Reasoning that produced nothing.",
        "rawResponses":["{\"choices\":[{\"delta\":{\"reasoning_content\":\"Reasoning that produced nothing.\"}}]}"],
        "newTokenCount":null,"snapshotBytes":null,"incompleteToolCalls":[],
        "finishReason":"STOP","usage":served(233926, 40, 40, 100)})
}

fn record(output: Value, rejected: Value) -> Value {
    json!({"type":"system","subtype":"compaction","parent_tool_use_id":null,
        "data":{"status":"COMPRESSION_FAILED_OUTPUT_TRUNCATED","succeeded":false,
            "originalTokenCount":233926,"newTokenCount":233926,"triggerReason":"token_limit",
            "output":output,"postCompactionHistory":null,"rejectedAttempts":rejected}})
}

/// Field-validation fixtures carry explicit physical draws. Their synthetic
/// utility requests are not evidence for the production compactor's ownership
/// or for resumed history; those paths need their own producer/replay tests.
fn trace_with(record: Value, child: bool) -> Trace {
    let mut trace = Trace::new();
    if child {
        trace.chat(None, "child");
    }
    for draw in record["data"]["rejectedAttempts"]
        .as_array()
        .unwrap()
        .iter()
        .chain(
            record["data"]["output"]
                .as_object()
                .map(|_| &record["data"]["output"]),
        )
    {
        let attempts = draw["requestAttempts"].as_u64().unwrap();
        for attempt in 0..attempts {
            trace.utility(
                if child {
                    "child-compaction"
                } else {
                    "root-compaction"
                },
                if attempt + 1 == attempts {
                    draw["usage"].clone()
                } else {
                    Value::Null
                },
            );
        }
    }
    let mut record = record;
    record["parent_tool_use_id"] = if child { json!("child") } else { Value::Null };
    trace.push(record);
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

fn commit_history(trace: &mut Trace, history: Value) {
    let data = data(trace);
    data["succeeded"] = json!(true);
    data["status"] = json!("COMPRESSED");
    data["output"]["finishReason"] = json!("STOP");
    data["output"]["incompleteToolCalls"] = json!([]);
    data["output"]["text"] = json!("accepted snapshot");
    data["output"]["rawResponses"] = json!(["{\"choices\":[{\"message\":{\"content\":\"accepted snapshot\"},\"finish_reason\":\"stop\"}]}"]);
    data["postCompactionHistory"] = history;
}

#[test]
fn compaction_preserves_served_interrupted_and_unattempted_observations() {
    let mut interrupted = output();
    interrupted["usage"] = Value::Null;
    interrupted["finishReason"] = Value::Null;
    for observation in [output(), interrupted, Value::Null] {
        let trace = trace_with(record(observation, json!([])), false);
        trace.certify();
        assert_eq!(observed_usage(&trace.snapshot()), trace.summary(None));
    }
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
        let mut success = trace_with(record(output(), json!([])), child);
        commit_history(
            &mut success,
            json!([{"role":"user","parts":[{"text":"accepted snapshot"}]}]),
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
fn compaction_draws_require_content_measurements_and_provider_objects() {
    let complete = trace_with(record(output(), json!([])), false);
    complete.certify();
    for field in [
        "rawResponses",
        "text",
        "newTokenCount",
        "snapshotBytes",
        "usage",
        "requestAttempts",
        "finishReason",
    ] {
        let mut trace = complete.clone();
        data(&mut trace)["output"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_compaction_refusal(
            &trace,
            if ["rawResponses", "text", "newTokenCount", "snapshotBytes"].contains(&field) {
                "schema rule /oneOf"
            } else {
                field
            },
        );
    }
    for (responses, expected) in [
        (json!([]), "schema rule /oneOf"),
        (json!(["not JSON"]), "undecodable raw response"),
        (json!(["null"]), "non-object raw response"),
    ] {
        let mut trace = complete.clone();
        data(&mut trace)["output"]["rawResponses"] = responses;
        assert_compaction_refusal(&trace, expected);
    }
    let mut trace = complete;
    data(&mut trace)["output"]["unknown_evidence"] = json!(true);
    assert_compaction_refusal(&trace, "violates stream contract");
}

#[test]
fn compaction_history_is_required_exactly_when_committed() {
    let mut trace = trace_with(record(output(), json!([])), false);
    trace.certify();
    commit_history(&mut trace, Value::Null);
    assert_compaction_refusal(&trace, "schema rule /oneOf");
    data(&mut trace)["postCompactionHistory"] = json!([
        {"role":"user","parts":[{"text":"<all_user_messages>\n"},
            {"text":"Original user text, verbatim.\n"},{"text":"</all_user_messages>"}]},
        {"role":"model","parts":[{"functionCall":{"id":"carried","name":"read_file","args":{"path":"a"}}}]}
    ]);
    trace.certify();
    data(&mut trace)["succeeded"] = json!(false);
    data(&mut trace)["status"] = json!("COMPRESSION_FAILED_HISTORY_CHANGED");
    assert_compaction_refusal(&trace, "schema rule /oneOf");
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
        ("status", Value::Null, "status"),
        ("requestAttempts", json!(0), "reports no request attempt"),
        ("incompleteToolCalls", Value::Null, "incompleteToolCalls"),
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
        assert_compaction_refusal(&trace, field);
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
        ("reasoning", Value::Null, "reasoning"),
        ("text", Value::Null, "text"),
        ("requestAttempts", json!(0), "reports no request attempt"),
        ("maxOutputTokens", json!(0), "positive budget"),
        ("usage", json!(7), "usage"),
        ("incompleteToolCalls", Value::Null, "incompleteToolCalls"),
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
    assert_compaction_refusal(&trace, "promptTokenCount");
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
            "requestAttempts",
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
            } else if ["maxOutputTokens", "requestAttempts"].contains(&field) {
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
    let max = runtime_contract::SAFE_INTEGER;
    for child in [false, true] {
        let mut trace = trace_with(record(output(), json!([])), child);
        let data = data(&mut trace);
        data["originalTokenCount"] = json!(max);
        data["newTokenCount"] = json!(max);
        data["output"]["maxOutputTokens"] = json!(max);
        data["output"]["newTokenCount"] = json!(max);
        data["output"]["snapshotBytes"] = json!(max);
        trace.certify();
    }
    let mut draw = output();
    draw["requestAttempts"] = json!(1);
    draw["maxOutputTokens"] = json!(max);
    draw["usage"] = served(max - 1, 1, 1, max - 1);
    trace_with(record(draw, json!([])), false).certify();
}
