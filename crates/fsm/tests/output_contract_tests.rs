// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Engine-level acceptance for output-contract rejection tracing, driven
//! through the real `ProfileFsmEngine` against a real on-disk ledger — the
//! same shape `approval_engine_tests.rs` uses.
//!
//! A final-macro submit that fails output-contract validation (missing
//! output, or a schema mismatch) used to return the error before any
//! ledger append happened, so the ledger recorded nothing for the
//! rejection — unlike every other rejection path (`MilestoneRejected`,
//! `ApprovalRejected`, `CircuitBreakerTriggered`), all of which append a
//! typed event. This test pins down that a rejected submit now leaves an
//! `OutputContractViolated` event, leaves the session `Active` (re-tryable,
//! not wedged), and that a conforming resubmit still completes normally.

use protocol_fsm::{FsmConfig, ProfileFsmEngine, StepOutcome};
use protocol_library::Library;
use protocol_types::{
    ChecklistEvidence, FsmError, FsmEventType, OutputContract, Position, Profile, ProfileSettings,
    SessionStatus, StateDef, SubStateDef, SubStateType,
};
use std::collections::HashMap;
use std::path::Path;

const SESSION: &str = "sess-output-contract";

fn sub_state(id: &str, kind: SubStateType) -> SubStateDef {
    SubStateDef {
        id: id.to_string(),
        sub_state_type: kind,
        name: id.to_string(),
        description: String::new(),
        enabled: true,
        criteria: None,
        inject: None,
        verify: None,
        approver_pubkey: None,
        approval_prompt: None,
        hooks: Vec::new(),
    }
}

fn checklist(id: &str, criterion: &str) -> SubStateDef {
    SubStateDef {
        criteria: Some(vec![criterion.to_string()]),
        ..sub_state(id, SubStateType::Checklist)
    }
}

fn macro_state(state_id: &str, sub_states: Vec<SubStateDef>) -> StateDef {
    StateDef {
        state_id: state_id.to_string(),
        name: state_id.to_string(),
        description: String::new(),
        system_prompt: None,
        enabled: true,
        max_iterations: 9,
        loop_state: false,
        icon: None,
        sub_states,
        hooks: Vec::new(),
    }
}

/// A single final macro (`ship`) whose checklist is also the output
/// contract's completion point. The contract requires `output.result` to
/// be a string.
fn profile() -> Profile {
    Profile {
        name: "output-contract-test".to_string(),
        version: "1.0.0".to_string(),
        description: String::new(),
        protected: false,
        created_at: String::new(),
        cloned_from: None,
        settings: ProfileSettings::default(),
        output_contract: Some(OutputContract {
            format: "json".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "required": ["result"],
                "properties": { "result": { "type": "string" } }
            }),
            destination: "{session_id}/output.json".to_string(),
        }),
        pipeline: vec![macro_state("ship", vec![checklist("chk", "shipped")])],
    }
}

fn engine_on(dir: &Path) -> ProfileFsmEngine<protocol_ledger::Ledger> {
    let library = Library::new(dir.join("library"));
    let ledger = protocol_ledger::Ledger::new(SESSION, dir).expect("ledger");
    let config = FsmConfig {
        output_base: dir.to_path_buf(),
        ..Default::default()
    };
    ProfileFsmEngine::new(profile(), library, ledger, config)
}

fn evidence(pairs: &[(&str, serde_json::Value)]) -> ChecklistEvidence {
    let mut m: ChecklistEvidence = HashMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    m
}

fn chk_position() -> Position {
    Position {
        macro_id: "ship".to_string(),
        sub_state_id: "chk".to_string(),
    }
}

#[test]
fn output_contract_violation_lands_in_ledger_and_session_stays_active() {
    let tmp = tempfile::tempdir().unwrap();
    let mut engine = engine_on(tmp.path());
    engine.start_session(SESSION, None).expect("start");

    // "result" must be a string per the schema; submitting a number is a
    // schema-validation failure (not a missing-output failure).
    let result = engine.submit_milestone(
        SESSION,
        chk_position(),
        evidence(&[("shipped", serde_json::json!(true))]),
        Some(serde_json::json!({ "result": 42 })),
    );
    assert!(
        matches!(result, Err(FsmError::OutputContractViolation(_))),
        "expected OutputContractViolation, got {result:?}"
    );

    // The session stays on the final checklist, exactly as every other
    // rejection path leaves it — the submit is re-tryable.
    assert_eq!(
        engine.get_state(SESSION).unwrap().status,
        SessionStatus::Active,
        "a contract-rejected submit must leave the session Active, not Completed/Failed"
    );

    let events = protocol_ledger::Ledger::replay(SESSION, tmp.path()).expect("replay");
    let violations: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.event_type, FsmEventType::OutputContractViolated { .. }))
        .collect();
    assert_eq!(
        violations.len(),
        1,
        "exactly one OutputContractViolated event must be appended; got events: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.event_type, FsmEventType::SessionCompleted { .. })),
        "a contract-rejected submit must never append SessionCompleted"
    );

    // A conforming resubmit still completes normally — the violation event
    // does not wedge the session.
    let retry = engine
        .submit_milestone(
            SESSION,
            chk_position(),
            evidence(&[("shipped", serde_json::json!(true))]),
            Some(serde_json::json!({ "result": "ok" })),
        )
        .unwrap();
    assert!(
        matches!(retry, StepOutcome::Completed { .. }),
        "expected Completed on the conforming resubmit, got {retry:?}"
    );
}
