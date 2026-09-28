use super::*;

#[test]
fn rehashed_generation_thought_cannot_certify_without_its_physical_source() {
    let mut trace = Trace::new();
    trace.chat(None, "call-x");
    trace.terminal(None, 1, None);
    trace.certify();

    let generation = trace
        .rows
        .iter_mut()
        .find(|row| row["type"] == "model_generation")
        .unwrap();
    let evidence = &mut generation["generation"];
    let mut envelope: Value =
        serde_json::from_str(evidence["generation_json"].as_str().unwrap()).unwrap();
    envelope["observations"][0]["response"]["candidates"][0]["content"]["parts"][0]["text"] =
        json!("  **forged thought**\\n");
    let text = envelope.to_string();
    let digest = hash(text.as_bytes());
    evidence["generation_json"] = json!(text);
    evidence["generation_bytes"] = json!(text.len());
    evidence["generation_sha256"] = json!(digest);
    let completion = trace
        .rows
        .iter_mut()
        .find(|row| row["type"] == "model_attempt_completion")
        .unwrap();
    completion["completion"]["generation_sha256"] = json!(digest);

    assert_refused_at(&trace, "physical response replay refused");
}

#[test]
fn refuses_missing_or_corrupted_raw_response_evidence() {
    let complete = Trace::ordinary();
    complete.certify();
    for defect in [
        "omission", "gap", "base64", "hash", "size", "identity", "unknown", "open",
    ] {
        let mut damaged = complete.clone();
        let body = damaged
            .rows
            .iter()
            .position(|row| row["response"]["event"]["kind"] == "body")
            .unwrap();
        let end = body + 1;
        match defect {
            "omission" => {
                damaged.rows.remove(end);
            }
            "gap" => damaged.rows[body]["response"]["event"]["offset"] = json!(1),
            "base64" => damaged.rows[body]["response"]["event"]["base64"] = json!("e30=\n"),
            "hash" => damaged.rows[end]["response"]["event"]["body_sha256"] = json!("0".repeat(64)),
            "size" => damaged.rows[end]["response"]["event"]["body_bytes"] = json!(0),
            "identity" => damaged.rows[body]["response"]["request_id"] = json!("unissued"),
            "unknown" => damaged.rows[body]["response"]["event"]["kind"] = json!("unknown"),
            "open" => {
                damaged.rows.last_mut().unwrap()["request_evidence"]["open_response_ids"] =
                    json!(["request-1"])
            }
            _ => unreachable!(),
        }
        assert!(damaged.snapshot().certified.is_err(), "{defect}");
    }
}

#[test]
fn completed_response_counts_match_physical_sdk_values() {
    for (mut trace, expected) in [(Trace::ordinary(), 1_u64), (Trace::delegated(), 3_u64)] {
        trace.certify();
        let outcome = trace
            .rows
            .iter_mut()
            .find(|row| {
                row["response"]["event"]["kind"] == "outcome"
                    && row["response"]["event"]["sdk_values_seen"].as_u64() == Some(expected)
            })
            .unwrap();
        let seen = outcome["response"]["event"]["sdk_values_seen"]
            .as_u64()
            .unwrap();
        outcome["response"]["event"]["sdk_values_seen"] = json!(seen + 1);
        assert_refused_at(
            &trace,
            "SDK value count claims an impossible physical response prefix",
        );
    }
    let mut trace = Trace::delegated();
    let outcome = trace
        .rows
        .iter_mut()
        .find(|row| {
            row["response"]["event"]["kind"] == "outcome"
                && row["response"]["event"]["sdk_values_seen"].as_u64() == Some(3)
        })
        .unwrap();
    outcome["response"]["event"]["sdk_values_seen"] = json!(2);
    assert_refused_at(
        &trace,
        "SDK value count claims an impossible physical response prefix",
    );
}

#[test]
fn refuses_missing_or_corrupted_request_evidence() {
    let complete = Trace::ordinary();
    complete.certify();
    for defect in ["omission", "hash", "kind", "origin", "sequence"] {
        let mut damaged = complete.clone();
        let request = damaged
            .rows
            .iter()
            .position(|row| row["type"] == "model_request")
            .unwrap();
        match defect {
            "omission" => {
                damaged.rows.remove(request);
                damaged.rows.last_mut().unwrap()["request_evidence"]["request_count"] = json!(0);
            }
            "hash" => damaged.rows[request]["request"]["body_sha256"] = json!("0".repeat(64)),
            "kind" => damaged.rows[request]["request"]["body"]["kind"] = json!("unknown"),
            "origin" => {
                damaged.rows[0]
                    .as_object_mut()
                    .unwrap()
                    .remove("request_evidence_origin");
            }
            "sequence" => damaged.rows[request]["request"]["sequence"] = json!(2),
            _ => unreachable!(),
        }
        assert!(damaged.snapshot().certified.is_err(), "{defect}");
    }
}

#[test]
fn captured_generation_requires_its_logical_completion_and_physical_evidence() {
    let mut complete = Trace::new();
    complete.chat(None, "call");
    complete.terminal(None, 1, None);
    complete.certify();
    for kind in ["model_generation", "model_attempt_completion"] {
        let mut damaged = complete.clone();
        damaged.rows.retain(|row| row["type"] != kind);
        assert!(damaged.snapshot().certified.is_err(), "missing {kind}");
    }
    for field in ["generation_sha256", "generation_id", "request_ids"] {
        let mut damaged = complete.clone();
        let completion = &mut damaged
            .rows
            .iter_mut()
            .find(|row| row["type"] == "model_attempt_completion")
            .unwrap()["completion"];
        completion[field] = match field {
            "generation_sha256" => json!("0".repeat(64)),
            "generation_id" => json!("unrecorded"),
            "request_ids" => json!(["unrecorded"]),
            _ => unreachable!(),
        };
        assert!(damaged.snapshot().certified.is_err(), "{field}");
    }
}

#[test]
fn reported_turns_and_terminal_states_survive_without_inferred_request_counts() {
    for turns in [0, 1, 2, 3, runtime_contract::SAFE_INTEGER] {
        for ending in std::iter::once(None).chain(
            ERROR_SUBTYPES
                .iter()
                .map(|subtype| Some((*subtype, "Ending description, verbatim."))),
        ) {
            let mut trace = Trace::new();
            trace.utility("a", ordinary_usage());
            trace.terminal(None, turns, ending);
            let terminal = trace.rows.last_mut().unwrap();
            terminal["duration_ms"] = json!(19_180_714);
            terminal["duration_api_ms"] = json!(18_037_819);
            let snapshot = trace.snapshot();
            assert_eq!(snapshot.observed.num_turns, Some(turns));
            let result = snapshot.certified.unwrap();
            assert_eq!(result.num_turns, turns);
            assert_eq!(result.usage.requests, 1);
            assert_eq!(result.duration_ms, 19_180_714);
            assert_eq!(result.api_duration_ms, 18_037_819);
            assert_eq!(result.is_error, ending.is_some());
            assert_eq!(
                result.subtype,
                ending.map_or(SUCCESS_SUBTYPE, |(name, _)| name)
            );
            assert_eq!(result.response, ending.map_or("ok", |(_, message)| message));
            assert!(result.scopes.is_empty());
        }
    }
}

#[test]
fn terminal_flags_and_names_form_a_closed_table() {
    Trace::ordinary().certify();
    for (is_error, subtype) in [
        (true, "error_incomplete"),
        (true, "success"),
        (false, "error_incomplete_generation"),
        (false, "error_slipped_final_message"),
    ] {
        let mut trace = Trace::ordinary();
        let terminal = trace.rows.last_mut().unwrap();
        terminal["subtype"] = json!(subtype);
        terminal["is_error"] = json!(is_error);
        if is_error {
            terminal["error"] = json!({"message":"boom"});
        }
        assert_refused_at(&trace, "schema rule /definitions/terminalOutcome/oneOf");
    }
}

#[test]
fn root_durations_and_turns_require_exact_counts() {
    Trace::ordinary().certify();
    for field in ["duration_ms", "duration_api_ms", "num_turns"] {
        for bad in [Value::Null, json!(-1), json!(1.5), json!("1"), json!(false)] {
            let mut trace = Trace::ordinary();
            trace.rows.last_mut().unwrap()[field] = bad;
            assert_refused_at(&trace, field);
        }
        let mut trace = Trace::ordinary();
        trace
            .rows
            .last_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_refused_at(&trace, "/definitions/terminalResult/required");
    }
}

#[test]
fn reported_turns_use_the_shared_exact_integer_domain_in_every_scope() {
    for child in [false, true] {
        let mut trace = if child {
            Trace::delegated()
        } else {
            Trace::ordinary()
        };
        let terminal = trace.rows.len() - if child { 2 } else { 1 };
        trace.rows[terminal]["num_turns"] = json!(runtime_contract::SAFE_INTEGER);
        trace.certify();
        trace.rows[terminal]["num_turns"] = json!(runtime_contract::SAFE_INTEGER + 1);
        assert_refused_at(&trace, "num_turns is not an admitted non-negative integer");
    }
}

#[test]
fn child_results_preserve_reported_claims_without_terminating_the_root() {
    for turns in [0, 3] {
        let mut trace = Trace::delegated();
        let child = trace.rows.len() - 2;
        trace.rows[child]["num_turns"] = json!(turns);
        let result = trace.certify();
        assert!(!result.is_error);
        assert_eq!(result.response, "before  after");
        assert_eq!(result.num_turns, 1);
        assert_eq!(result.scopes.len(), 1);
        let scope = &result.scopes[0];
        assert_eq!(scope.tool_use_id, "child");
        assert_eq!(scope.tool_name, "audit_probe");
        assert_eq!(scope.reported_num_turns, Some(turns));
        assert_eq!(scope.is_error, Some(true));
        assert_eq!(scope.subtype.as_deref(), Some("error_during_execution"));
        assert_eq!(scope.error_message.as_deref(), Some("MAX_TURNS"));
        assert_eq!(
            result
                .request_scopes
                .iter()
                .find(|scope| scope.kv_scope == "child")
                .unwrap()
                .usage
                .requests,
            1
        );
        trace.rows.pop();
        assert_refused_at(&trace, "no main-session terminal result");
    }
}

#[test]
fn child_generations_keep_global_physical_evidence_and_local_tool_ancestry() {
    let mut trace = Trace::new();
    trace.chat(None, "child");
    trace.chat(Some("child"), "grandchild");
    trace.presentation(Some("grandchild"));
    trace.terminal(Some("grandchild"), 0, None);
    trace.terminal(Some("child"), 1, None);
    trace.terminal(None, 1, None);
    let result = trace.certify();
    assert_eq!(result.usage.requests, 2);
    assert_eq!(result.scopes.len(), 2);
    assert_eq!(result.scopes[0].tool_use_id, "child");
    assert_eq!(result.scopes[1].tool_use_id, "grandchild");
    assert_eq!(
        result
            .request_scopes
            .iter()
            .map(|row| row.kv_scope.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "child"]
    );
    assert!(trace
        .rows
        .iter()
        .filter(|row| matches!(
            row["type"].as_str(),
            Some("model_request" | "model_response")
        ))
        .all(|row| row["parent_tool_use_id"].is_null()));
}

#[test]
fn independent_children_keep_distinct_nullable_terminal_claims() {
    let mut trace = Trace::new();
    trace.chat(None, "call-a");
    trace.utility("call-a", ordinary_usage());
    trace.terminal(Some("call-a"), 5, None);
    trace.chat(None, "call-b");
    trace.utility("call-b", ordinary_usage());
    trace.utility("call-b", ordinary_usage());
    trace.presentation(Some("call-b"));
    trace.terminal(None, 2, None);
    let result = trace.certify();
    assert_eq!(result.scopes.len(), 2);
    assert_eq!(result.scopes[0].tool_use_id, "call-a");
    assert_eq!(result.scopes[0].reported_num_turns, Some(5));
    assert_eq!(result.scopes[0].is_error, Some(false));
    assert_eq!(result.scopes[0].subtype.as_deref(), Some("success"));
    assert_eq!(result.scopes[0].error_message, None);
    assert_eq!(result.scopes[1].tool_use_id, "call-b");
    assert_eq!(result.scopes[1].reported_num_turns, None);
    assert_eq!(result.scopes[1].is_error, None);
    assert_eq!(result.scopes[1].subtype, None);
    assert_eq!(result.scopes[1].error_message, None);
    for (owner, count) in [("a", 2), ("call-a", 1), ("call-b", 2)] {
        assert_eq!(
            result
                .request_scopes
                .iter()
                .find(|scope| scope.kv_scope == owner)
                .unwrap()
                .usage
                .requests,
            count
        );
    }
}

#[test]
fn orphan_and_future_child_owners_are_refused_at_their_first_reference() {
    Trace::delegated().certify();
    for future in [false, true] {
        let mut trace = Trace::new();
        trace.presentation(Some("child"));
        let orphan_line = trace
            .rows
            .iter()
            .position(|row| row["subtype"] == "runtime_operation")
            .unwrap()
            + 1;
        if future {
            trace.chat(None, "child");
        }
        trace.terminal(None, 1, None);
        assert_refused_at(
            &trace,
            &format!("line {orphan_line} names parent_tool_use_id \"child\", which no accepted generation issued"),
        );
    }
}

#[test]
fn terminal_scope_and_root_boundaries_refuse_later_events() {
    let complete = Trace::delegated();
    complete.certify();
    for second_result in [false, true] {
        let mut trace = complete.clone();
        trace.rows.pop();
        let terminal_line = trace.rows.len();
        if second_result {
            trace.terminal(Some("child"), 3, None);
        } else {
            trace.presentation(Some("child"));
        }
        trace.terminal(None, 1, None);
        assert_refused_at(
            &trace,
            &format!("after its terminal result at line {terminal_line}"),
        );
    }
    for child in [false, true] {
        let mut trace = complete.clone();
        trace.terminal(if child { Some("child") } else { None }, 1, None);
        assert_refused_at(
            &trace,
            &format!(
                "terminal result at line {} is followed by another event",
                complete.rows.len()
            ),
        );
    }
    let mut trace = complete.clone();
    trace.push(complete.rows[0].clone());
    assert_refused_at(&trace, "is followed by another event");
}

#[test]
fn duplicate_accepted_tool_ids_are_refused_across_generations() {
    let mut control = Trace::new();
    control.chat(None, "first");
    control.chat(None, "second");
    control.terminal(None, 2, None);
    control.certify();
    let mut trace = Trace::new();
    trace.chat(None, "duplicate");
    trace.chat(None, "duplicate");
    trace.terminal(None, 2, None);
    assert_refused_at(&trace, "re-issues tool_use id \"duplicate\"");
}

#[test]
fn malformed_parent_identity_is_never_a_scope() {
    let mut complete = Trace::new();
    complete.presentation(None);
    complete.terminal(None, 0, None);
    complete.certify();
    for bad in [json!(0), json!(""), json!([]), json!(false), json!({})] {
        let mut trace = complete.clone();
        let presentation = trace
            .rows
            .iter_mut()
            .find(|row| row["type"] == "assistant")
            .unwrap();
        presentation["parent_tool_use_id"] = bad;
        assert_refused_at(&trace, "parent_tool_use_id");
    }
}

#[test]
fn request_usage_is_partitioned_by_kv_owner_without_billing_presentation() {
    let mut trace = Trace::new();
    trace.chat(None, "child"); // Captured report: 7 output, 2 thoughts.
    trace.utility("a", served(100, 99, 73, 0));
    trace.utility("child", served(900, 40, 33, 896));
    trace.presentation(Some("child"));
    trace.utility("child", served(1200, 25, 0, 1200));
    trace.utility("internal", Value::Null);
    trace.terminal(None, 2, None);
    let result = trace.certify();
    for (scope, output, thoughts) in [("a", 106, 75), ("child", 65, 33)] {
        let row = result
            .request_scopes
            .iter()
            .find(|row| row.kv_scope == scope)
            .unwrap();
        assert_eq!(row.usage.requests, 2);
        assert_eq!(row.usage.usage.unwrap().output, output);
        assert_eq!(row.usage.usage.unwrap().thoughts, thoughts);
    }
    let internal = result
        .request_scopes
        .iter()
        .find(|row| row.kv_scope == "internal")
        .unwrap();
    assert_eq!(internal.usage.unreported_usage_requests, 1);
    assert_eq!(internal.usage.usage, None);
    assert_eq!(result.scopes.len(), 1);
    assert_eq!(result.usage.requests, 5);
    assert_eq!(result.usage.usage.unwrap().output, 171);
}

#[test]
fn no_dispatch_zero_usage_unknown_usage_and_pending_are_distinct() {
    for usage in [
        None,
        Some(Value::Null),
        Some(served(0, 0, 0, 0)),
        Some(ordinary_usage()),
    ] {
        let mut trace = Trace::new();
        if let Some(usage) = usage {
            trace.utility("a", usage);
        }
        trace.terminal(None, 0, None);
        let snapshot = trace.snapshot();
        assert_eq!(observed_usage(&snapshot), trace.summary(None));
        assert_eq!(
            serde_json::to_value(snapshot.certified.unwrap().usage).unwrap(),
            trace.summary(None)
        );
        assert_eq!(snapshot.observed.observed_unaccounted_records, 0);
    }
    let mut trace = Trace::new();
    trace.pending("a", None);
    assert_eq!(
        trace.snapshot().observed.observed_usage,
        GenerationUsageSummary {
            requests: 1,
            unfinalized_requests: 1,
            ..GenerationUsageSummary::default()
        }
    );
    trace.terminal(None, 1, Some(("error_during_execution", "interrupted")));
    assert_refused_at(&trace, "every physical request and logical attempt");
}

fn invalid_usage_cases() -> Vec<Value> {
    let mut cases = vec![json!({}), json!(0), json!([]), json!(false)];
    for field in [
        "promptTokenCount",
        "candidatesTokenCount",
        "thoughtsTokenCount",
        "cachedContentTokenCount",
        "totalTokenCount",
    ] {
        let mut missing = ordinary_usage();
        missing.as_object_mut().unwrap().remove(field);
        cases.push(missing);
        for value in [
            Value::Null,
            json!(-1),
            json!(0.5),
            json!("0"),
            json!(false),
            json!(runtime_contract::SAFE_INTEGER + 1),
        ] {
            let mut bad = ordinary_usage();
            bad[field] = value;
            cases.push(bad);
        }
    }
    for (field, value) in [
        ("thoughtsTokenCount", 10),
        ("cachedContentTokenCount", 43),
        ("totalTokenCount", 50),
        ("totalTokenCount", 57),
    ] {
        let mut bad = ordinary_usage();
        bad[field] = json!(value);
        cases.push(bad);
    }
    let mut unknown = ordinary_usage();
    unknown["unrecognized"] = json!(0);
    cases.push(unknown);
    cases
}

#[test]
fn unreadable_outcome_usage_stays_pending_between_usable_reports() {
    let mut complete = Trace::new();
    complete.utility("a", ordinary_usage());
    complete.utility("a", ordinary_usage());
    let middle = complete
        .rows
        .iter()
        .rposition(|row| row["response"]["event"]["kind"] == "outcome")
        .unwrap();
    complete.utility("a", ordinary_usage());
    complete.terminal(None, 1, None);
    complete.certify();
    let expected = GenerationUsageSummary {
        requests: 3,
        usage_reports: 2,
        unfinalized_requests: 1,
        unreported_usage_requests: 0,
        usage: Some(ServedUsage {
            prompt: 84,
            output: 18,
            thoughts: 12,
            cached: 0,
            total: 102,
        }),
    };
    for bad in invalid_usage_cases() {
        let mut trace = complete.clone();
        trace.rows[middle]["response"]["event"]["served_usage"] = bad.clone();
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.observed.observed_usage, expected, "{bad}");
        assert_eq!(snapshot.observed.num_turns, Some(1));
        assert_eq!(snapshot.observed.observed_unaccounted_records, 1, "{bad}");
        assert!(snapshot.certified.is_err(), "{bad}");
    }
}

#[test]
fn invalid_outer_identity_cannot_attribute_an_outcome_or_a_child_scope() {
    let mut complete = Trace::new();
    complete.utility("a", ordinary_usage());
    complete.utility("a", ordinary_usage());
    let middle = complete
        .rows
        .iter()
        .rposition(|row| row["response"]["event"]["kind"] == "outcome")
        .unwrap();
    complete.utility("a", ordinary_usage());
    complete.terminal(None, 1, None);
    complete.certify();
    for (key, replacement) in [
        ("session_id", None),
        ("session_id", Some(json!("foreign"))),
        ("session_id", Some(json!(""))),
        ("session_id", Some(json!(7))),
        ("uuid", None),
        ("uuid", Some(json!(""))),
        ("uuid", Some(json!(7))),
        ("uuid", Some(complete.rows[1]["uuid"].clone())),
    ] {
        let mut trace = complete.clone();
        let row = &mut trace.rows[middle];
        row["parent_tool_use_id"] = json!("unissued-child");
        if let Some(value) = replacement {
            row[key] = value;
        } else {
            row.as_object_mut().unwrap().remove(key);
        }
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.observed.observed_usage.requests, 3);
        assert_eq!(snapshot.observed.observed_usage.usage_reports, 2);
        assert_eq!(snapshot.observed.observed_usage.unfinalized_requests, 1);
        assert_eq!(snapshot.observed.observed_usage.usage.unwrap().output, 18);
        assert_eq!(snapshot.observed.observed_subagent_scope_count, 0);
        assert_eq!(snapshot.observed.observed_unaccounted_records, 1);
        assert!(snapshot.certified.is_err());
    }
}

#[test]
fn absent_or_mismatched_stream_identity_cannot_attribute_served_usage() {
    Trace::ordinary().certify();
    for absent in [false, true] {
        let mut trace = Trace::ordinary();
        if absent {
            trace.rows.remove(0);
        } else {
            trace.rows[0]["stream_contract_sha256"] = json!("0".repeat(64));
        }
        let snapshot = trace.snapshot();
        assert_eq!(
            snapshot.observed.observed_usage,
            GenerationUsageSummary::default()
        );
        assert_eq!(snapshot.observed.num_turns, None);
        assert_eq!(snapshot.observed.observed_subagent_scope_count, 0);
        assert_eq!(
            snapshot.observed.observed_unaccounted_records,
            trace.rows.len() as u64
        );
        assert!(snapshot.certified.is_err());
    }
}

#[test]
fn missing_or_mismatched_runtime_metadata_refuses_certification_but_retains_observed_cost() {
    for defect in ["missing", "model", "slash_commands"] {
        let mut trace = Trace::ordinary();
        match defect {
            "missing" => {
                trace.rows.remove(1);
            }
            "model" => trace.rows[1]["model"] = json!("foreign-model"),
            "slash_commands" => trace.rows[1]["slash_commands"] = json!(["status"]),
            _ => unreachable!(),
        }
        let snapshot = trace.snapshot();
        assert!(snapshot.certified.is_err(), "{defect}");
        assert_eq!(observed_usage(&snapshot), trace.summary(None), "{defect}");
        assert_eq!(snapshot.observed.num_turns, Some(1), "{defect}");
    }
}

fn reported_generation_summary() -> Value {
    Trace::ordinary().summary(None)
}

fn assert_generation_summaries_refused(cases: Vec<(String, Value)>) {
    let complete = Trace::ordinary();
    complete.certify();
    for (label, usage) in cases {
        let mut trace = complete.clone();
        trace.rows.last_mut().unwrap()["usage"] = usage;
        let snapshot = trace.snapshot();
        assert_eq!(observed_usage(&snapshot), complete.summary(None), "{label}");
        assert_eq!(
            snapshot.observed.output_event_bytes,
            trace.text().len() as u64
        );
        assert!(snapshot.certified.is_err(), "{label}");
    }
}

#[test]
fn terminal_summary_requires_a_complete_object_and_valid_partition() {
    let mut cases = vec![
        Value::Null,
        json!(0),
        json!(false),
        json!("usage"),
        json!([]),
        json!({}),
        json!({"input_tokens":42,"output_tokens":9,"reasoning_output_tokens":6,"cache_read_input_tokens":0,"total_tokens":51}),
    ];
    for (field, value) in [
        ("requests", 0),
        ("requests", 2),
        ("usageReports", 2),
        ("unfinalizedRequests", 1),
        ("unreportedUsageRequests", 1),
    ] {
        let mut summary = reported_generation_summary();
        summary[field] = json!(value);
        cases.push(summary);
    }
    for field in [
        "requests",
        "usageReports",
        "unfinalizedRequests",
        "unreportedUsageRequests",
        "usage",
    ] {
        let mut missing = reported_generation_summary();
        missing.as_object_mut().unwrap().remove(field);
        cases.push(missing);
        for value in [
            Value::Null,
            json!(-1),
            json!(1.5),
            json!("1"),
            json!(false),
            json!(runtime_contract::SAFE_INTEGER + 1),
        ] {
            let mut summary = reported_generation_summary();
            summary[field] = value;
            cases.push(summary);
        }
    }
    for bad in invalid_usage_cases() {
        // Terminal served metadata is open; an unknown count in a physical
        // outcome is closed and is covered by the outcome test above.
        if bad.get("unrecognized").is_some() {
            continue;
        }
        let mut summary = reported_generation_summary();
        summary["usage"] = bad;
        cases.push(summary);
    }
    for usage in [served(0, 0, 0, 0), ordinary_usage()] {
        cases.push(
            json!({"requests":0,"usageReports":0,"unfinalizedRequests":0,
            "unreportedUsageRequests":0,"usage":usage}),
        );
    }
    assert_generation_summaries_refused(
        cases
            .into_iter()
            .enumerate()
            .map(|(i, value)| (format!("case {i}: {value}"), value))
            .collect(),
    );
    let mut trace = Trace::ordinary();
    trace
        .rows
        .last_mut()
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("usage");
    assert_refused_at(&trace, "/definitions/terminalResult/required");
}

#[test]
fn a_well_formed_terminal_summary_must_equal_the_physical_record() {
    let mut false_total = reported_generation_summary();
    false_total["usage"] = served(100, 20, 15, 80);
    let max = runtime_contract::SAFE_INTEGER;
    assert_generation_summaries_refused(vec![
        ("invented tokens".into(), false_total),
        (
            "invented reports".into(),
            json!({"requests":max,"usageReports":max,
            "unfinalizedRequests":0,"unreportedUsageRequests":0,"usage":ordinary_usage()}),
        ),
        (
            "invented pending requests".into(),
            json!({"requests":max,"usageReports":0,
            "unfinalizedRequests":max,"unreportedUsageRequests":0,"usage":null}),
        ),
        (
            "invented unknown requests".into(),
            json!({"requests":max,"usageReports":0,
            "unfinalizedRequests":0,"unreportedUsageRequests":max,"usage":null}),
        ),
    ]);
}

#[test]
fn exact_safe_token_boundaries_require_an_actual_report() {
    let max = runtime_contract::SAFE_INTEGER;
    for usage in [served(max - 1, 1, 1, max - 1), served(0, max, max, 0)] {
        let mut trace = Trace::new();
        trace.utility("a", usage.clone());
        trace.terminal(None, 1, None);
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.observed.observed_usage.usage_reports, 1);
        assert_eq!(
            serde_json::to_value(snapshot.certified.unwrap().usage.usage).unwrap(),
            usage
        );
    }
}

#[test]
fn raw_numeric_lexemes_reach_the_exact_reader_without_reserialization() {
    let trace = Trace::ordinary();
    trace.certify();
    let outcome = trace
        .rows
        .iter()
        .position(|row| row["response"]["event"]["kind"] == "outcome")
        .unwrap();
    for (lexeme, accepted) in [
        ("9.0", true),
        ("9e0", true),
        ("9.0000000000000001", false),
        ("9.0000000000000000000000000000001", false),
    ] {
        let changed = trace.rows[outcome].to_string().replace(
            "\"candidatesTokenCount\":9",
            &format!("\"candidatesTokenCount\":{lexeme}"),
        );
        let bytes = format!(
            "{}{changed}\n{}",
            frames(&trace.rows[..outcome]),
            frames(&trace.rows[outcome + 1..])
        );
        let snapshot = snapshot_text(&bytes).unwrap();
        assert_eq!(snapshot.certified.is_ok(), accepted, "{lexeme}");
        assert_eq!(
            snapshot.observed.observed_usage.usage_reports,
            u64::from(accepted)
        );
        assert_eq!(
            snapshot.observed.observed_usage.unfinalized_requests,
            u64::from(!accepted)
        );
    }
}
