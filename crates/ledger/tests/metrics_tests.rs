// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Tests for `protocol_ledger::metrics`.
//! Deterministic: explicit `DateTime` timestamps only, never `Utc::now()`.

use chrono::DateTime;
use protocol_ledger::metrics::{aggregate_by_criteria_count, compute_session_metrics};
use protocol_types::{FsmEvent, FsmEventType};
use serde_json::json;
use uuid::Uuid;

/// Build an event with an explicit deterministic timestamp
/// (`"2026-01-01T00:00:0{offset_secs}Z"`).
fn ev(
    session_id: &str,
    offset_secs: i64,
    step_id: Option<&str>,
    event_type: FsmEventType,
) -> FsmEvent {
    let ts = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc)
        + chrono::Duration::seconds(offset_secs);
    FsmEvent {
        event_id: Uuid::nil(),
        timestamp: ts,
        session_id: session_id.to_string(),
        event_type,
        step_id: step_id.map(|s| s.to_string()),
        payload: json!({}),
    }
}

/// A 2-macro session: macro_a/checklist_a rejected twice then accepted,
/// then advances to macro_b/checklist_b which passes clean.
fn synthetic_session(session_id: &str, pipeline_id: &str) -> Vec<FsmEvent> {
    vec![
        ev(
            session_id,
            0,
            None,
            FsmEventType::SessionStarted {
                pipeline_id: pipeline_id.to_string(),
                manifest_version: "1".to_string(),
                profile_sha256: None,
            },
        ),
        ev(
            session_id,
            1,
            Some("macro_a/checklist_a"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["tests".to_string(), "docs".to_string()],
                reason: "missing tests".to_string(),
                iteration: 1,
            },
        ),
        ev(
            session_id,
            2,
            Some("macro_a/checklist_a"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["tests".to_string()],
                reason: "still missing tests".to_string(),
                iteration: 2,
            },
        ),
        ev(
            session_id,
            3,
            Some("macro_a/checklist_a"),
            FsmEventType::MilestoneAccepted {
                next_step_id: Some("macro_a/checklist_b".to_string()),
            },
        ),
        ev(
            session_id,
            4,
            Some("macro_a/checklist_b"),
            FsmEventType::MilestoneAccepted { next_step_id: None },
        ),
        ev(
            session_id,
            5,
            None,
            FsmEventType::MacroAdvanced {
                from_macro: "macro_a".to_string(),
                to_macro: "macro_b".to_string(),
            },
        ),
        ev(
            session_id,
            6,
            Some("macro_b/checklist_c"),
            FsmEventType::MilestoneAccepted { next_step_id: None },
        ),
        ev(
            session_id,
            10,
            None,
            FsmEventType::SessionCompleted {
                final_artifact_path: "/tmp/artifact.json".to_string(),
                output_sha256: None,
            },
        ),
    ]
}

#[test]
fn step_metrics_rejections_and_histogram() {
    let events = synthetic_session("sess-1", "pipeline-a");
    let metrics = compute_session_metrics(&events, None);

    assert_eq!(metrics.session_id, "sess-1");
    assert_eq!(metrics.pipeline_id.as_deref(), Some("pipeline-a"));
    assert_eq!(metrics.status, "completed");
    assert_eq!(metrics.steps.len(), 3);

    let step_a = &metrics.steps[0];
    assert_eq!(step_a.macro_id, "macro_a");
    assert_eq!(step_a.sub_state_id, "checklist_a");
    assert_eq!(step_a.rejections, 2);
    assert_eq!(step_a.retry_count, 2);
    assert!(step_a.passed);
    assert_eq!(step_a.rejected_criteria.get("tests"), Some(&2));
    assert_eq!(step_a.rejected_criteria.get("docs"), Some(&1));

    let step_b = &metrics.steps[1];
    assert_eq!(step_b.sub_state_id, "checklist_b");
    assert_eq!(step_b.rejections, 0);
    assert!(step_b.passed);
    assert!(step_b.rejected_criteria.is_empty());

    let step_c = &metrics.steps[2];
    assert_eq!(step_c.sub_state_id, "checklist_c");
    assert_eq!(step_c.rejections, 0);
    // Final position: SessionCompleted counts as "left the position".
    assert!(step_c.passed);

    assert_eq!(metrics.total_rejections, 2);
    assert_eq!(metrics.total_dwell_ms, Some(10_000));
}

#[test]
fn terminal_checklist_counts_as_passed_when_session_completed_shares_its_step_id() {
    // Regression: the engine records the terminal checklist's SessionCompleted
    // under the SAME step_id as the checklist itself. The "did it advance?"
    // check must recognize THAT specific event as a pass (it previously only
    // looked at events with a DIFFERENT step_id, so the final gate always read
    // as passed=false even on a completed session).
    //
    // Deliberately OMITS the `MilestoneAccepted` that a real engine run would
    // also record under this step_id: if it were present it would itself
    // satisfy the "did it advance?" check (it's checked first and lands at
    // the same step_id/offset), making the `SessionCompleted` arm dead for
    // this test's purposes. Dropping it proves the `SessionCompleted` clause
    // is independently load-bearing, not merely redundant with
    // `MilestoneAccepted`.
    let events = vec![
        ev(
            "sess-term",
            0,
            None,
            FsmEventType::SessionStarted {
                pipeline_id: "pipeline-a".to_string(),
                manifest_version: "1".to_string(),
                profile_sha256: None,
            },
        ),
        // Arrival into the terminal macro/checklist (this is the position's
        // first event, as the real engine records it via MacroAdvanced).
        ev(
            "sess-term",
            1,
            Some("build/gate"),
            FsmEventType::MacroAdvanced {
                from_macro: "design".to_string(),
                to_macro: "build".to_string(),
            },
        ),
        // The terminal checklist passes: SessionCompleted is recorded under
        // the SAME step_id as the checklist, with no preceding
        // `MilestoneAccepted` at this step_id -- SessionCompleted is the
        // ONLY event that can register this as "left the position".
        ev(
            "sess-term",
            5,
            Some("build/gate"),
            FsmEventType::SessionCompleted {
                final_artifact_path: "./out/sess-term/result.json".to_string(),
                output_sha256: None,
            },
        ),
    ];
    let metrics = compute_session_metrics(&events, None);
    assert_eq!(metrics.status, "completed");
    let gate = metrics
        .steps
        .iter()
        .find(|s| s.sub_state_id == "gate")
        .expect("gate step present");
    assert!(
        gate.passed,
        "terminal gate must read as passed on completion via SessionCompleted alone"
    );
    // Arrival at offset 1s, SessionCompleted at offset 5s -> 4000ms dwell.
    assert_eq!(gate.dwell_ms, Some(4000));
}

#[test]
fn step_metrics_active_session_has_no_terminal_status() {
    let events = vec![
        ev(
            "sess-active",
            0,
            None,
            FsmEventType::SessionStarted {
                pipeline_id: "pipeline-a".to_string(),
                manifest_version: "1".to_string(),
                profile_sha256: None,
            },
        ),
        ev(
            "sess-active",
            1,
            Some("macro_a/checklist_a"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["tests".to_string()],
                reason: "missing".to_string(),
                iteration: 1,
            },
        ),
    ];
    let metrics = compute_session_metrics(&events, None);
    assert_eq!(metrics.status, "active");
    assert_eq!(metrics.steps.len(), 1);
    assert!(!metrics.steps[0].passed);
    // The position never advances forward (session still stuck on it), so there
    // is no leave-event to measure a dwell to: dwell is None, not a fabricated 0.
    assert_eq!(metrics.steps[0].dwell_ms, None);
}

#[test]
fn step_metrics_failed_session_status() {
    let events = vec![
        ev(
            "sess-failed",
            0,
            None,
            FsmEventType::SessionStarted {
                pipeline_id: "pipeline-a".to_string(),
                manifest_version: "1".to_string(),
                profile_sha256: None,
            },
        ),
        ev(
            "sess-failed",
            1,
            Some("macro_a/checklist_a"),
            FsmEventType::SessionFailed {
                reason: "gave up".to_string(),
                final_step: "macro_a/checklist_a".to_string(),
            },
        ),
    ];
    let metrics = compute_session_metrics(&events, None);
    assert_eq!(metrics.status, "failed");
}

/// A `human_approval` gate's `ApprovalRejected` events must count into that
/// position's `rejections` (and its dedicated `approval_rejections`
/// counter) exactly like `MilestoneRejected` does for a checklist — they
/// were previously invisible to per-gate friction.
#[test]
fn approval_rejected_counts_into_rejections_and_approval_rejections() {
    let events = vec![
        ev(
            "sess-approval",
            0,
            None,
            FsmEventType::SessionStarted {
                pipeline_id: "pipeline-a".to_string(),
                manifest_version: "1".to_string(),
                profile_sha256: None,
            },
        ),
        ev(
            "sess-approval",
            1,
            Some("release/sign_off"),
            FsmEventType::ApprovalRejected {
                macro_id: "release".to_string(),
                sub_state_id: "sign_off".to_string(),
                reason: "bad signature".to_string(),
            },
        ),
        ev(
            "sess-approval",
            2,
            Some("release/sign_off"),
            FsmEventType::ApprovalRejected {
                macro_id: "release".to_string(),
                sub_state_id: "sign_off".to_string(),
                reason: "wrong approver".to_string(),
            },
        ),
        // A checklist rejection at the SAME step_id must still land only in
        // rejected_criteria/rejections, not approval_rejections.
        ev(
            "sess-approval",
            3,
            Some("release/sign_off"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["checklist_item".to_string()],
                reason: "not ready".to_string(),
                iteration: 1,
            },
        ),
        ev(
            "sess-approval",
            4,
            Some("release/sign_off"),
            FsmEventType::MilestoneAccepted { next_step_id: None },
        ),
        ev(
            "sess-approval",
            5,
            None,
            FsmEventType::SessionCompleted {
                final_artifact_path: "/tmp/artifact.json".to_string(),
                output_sha256: None,
            },
        ),
    ];

    let metrics = compute_session_metrics(&events, None);
    assert_eq!(metrics.steps.len(), 1);
    let gate = &metrics.steps[0];

    // 2 ApprovalRejected + 1 MilestoneRejected = 3 total rejections.
    assert_eq!(gate.rejections, 3);
    assert_eq!(gate.retry_count, 3);
    // Only the ApprovalRejected pair counted in the dedicated counter.
    assert_eq!(gate.approval_rejections, 2);
    // The checklist rejection's item still lands in the per-criterion histogram.
    assert_eq!(gate.rejected_criteria.get("checklist_item"), Some(&1));
    assert!(gate.passed);
    assert_eq!(metrics.total_rejections, 3);
}

/// Two sessions with checklist gates of size 3 and 5 -> two buckets with
/// correct means/first_pass_rate. `criteria_count` comes only from a
/// profile lookup, so we synthesize `SessionMetrics` via two sessions whose
/// step_ids map to sub_states carrying different criteria list lengths —
/// achieved here by loading a real profile is unnecessary for aggregation
/// itself: aggregate_by_criteria_count operates on already-computed
/// SessionMetrics, so we build them directly through compute_session_metrics
/// with a hand-built Profile.
#[test]
fn aggregate_by_criteria_count_buckets_correctly() {
    use protocol_types::{Profile, ProfileSettings, StateDef, SubStateDef, SubStateType};

    let make_sub = |id: &str, criteria: Vec<&str>| SubStateDef {
        id: id.to_string(),
        sub_state_type: SubStateType::Checklist,
        name: id.to_string(),
        description: String::new(),
        enabled: true,
        criteria: Some(criteria.into_iter().map(|s| s.to_string()).collect()),
        inject: None,
        verify: None,
        approver_pubkey: None,
        approval_prompt: None,
        hooks: Vec::new(),
    };

    let profile = Profile {
        name: "test-profile".to_string(),
        version: "1".to_string(),
        description: String::new(),
        protected: false,
        created_at: String::new(),
        cloned_from: None,
        settings: ProfileSettings::default(),
        pipeline: vec![
            StateDef {
                state_id: "macro_a".to_string(),
                name: "Macro A".to_string(),
                description: String::new(),
                system_prompt: None,
                enabled: true,
                max_iterations: 3,
                loop_state: false,
                icon: None,
                sub_states: vec![make_sub("gate_small", vec!["c1", "c2", "c3"])],
                hooks: Vec::new(),
            },
            StateDef {
                state_id: "macro_b".to_string(),
                name: "Macro B".to_string(),
                description: String::new(),
                system_prompt: None,
                enabled: true,
                max_iterations: 3,
                loop_state: false,
                icon: None,
                sub_states: vec![make_sub("gate_big", vec!["c1", "c2", "c3", "c4", "c5"])],
                hooks: Vec::new(),
            },
        ],
        output_contract: None,
    };

    // Session 1: gate_small (3 criteria) rejected once then passes;
    // gate_big (5 criteria) passes clean (0 rejections).
    let session1 = vec![
        ev(
            "sess-1",
            0,
            None,
            FsmEventType::SessionStarted {
                pipeline_id: "p".to_string(),
                manifest_version: "1".to_string(),
                profile_sha256: None,
            },
        ),
        ev(
            "sess-1",
            1,
            Some("macro_a/gate_small"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["c1".to_string()],
                reason: "r".to_string(),
                iteration: 1,
            },
        ),
        ev(
            "sess-1",
            2,
            Some("macro_a/gate_small"),
            FsmEventType::MilestoneAccepted { next_step_id: None },
        ),
        ev(
            "sess-1",
            3,
            Some("macro_b/gate_big"),
            FsmEventType::MilestoneAccepted { next_step_id: None },
        ),
        ev(
            "sess-1",
            4,
            None,
            FsmEventType::SessionCompleted {
                final_artifact_path: "/tmp/a".to_string(),
                output_sha256: None,
            },
        ),
    ];

    // Session 2: gate_small (3 criteria) passes clean;
    // gate_big (5 criteria) rejected 3 times then passes.
    let session2 = vec![
        ev(
            "sess-2",
            0,
            None,
            FsmEventType::SessionStarted {
                pipeline_id: "p".to_string(),
                manifest_version: "1".to_string(),
                profile_sha256: None,
            },
        ),
        ev(
            "sess-2",
            1,
            Some("macro_a/gate_small"),
            FsmEventType::MilestoneAccepted { next_step_id: None },
        ),
        ev(
            "sess-2",
            2,
            Some("macro_b/gate_big"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["c2".to_string()],
                reason: "r".to_string(),
                iteration: 1,
            },
        ),
        ev(
            "sess-2",
            3,
            Some("macro_b/gate_big"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["c2".to_string()],
                reason: "r".to_string(),
                iteration: 2,
            },
        ),
        ev(
            "sess-2",
            4,
            Some("macro_b/gate_big"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["c2".to_string()],
                reason: "r".to_string(),
                iteration: 3,
            },
        ),
        ev(
            "sess-2",
            5,
            Some("macro_b/gate_big"),
            FsmEventType::MilestoneAccepted { next_step_id: None },
        ),
        ev(
            "sess-2",
            6,
            None,
            FsmEventType::SessionCompleted {
                final_artifact_path: "/tmp/b".to_string(),
                output_sha256: None,
            },
        ),
    ];

    let m1 = compute_session_metrics(&session1, Some(&profile));
    let m2 = compute_session_metrics(&session2, Some(&profile));

    let buckets = aggregate_by_criteria_count(&[m1, m2]);
    assert_eq!(buckets.len(), 2);

    let bucket_3 = buckets.iter().find(|b| b.criteria_count == 3).unwrap();
    assert_eq!(bucket_3.gate_count, 2);
    assert_eq!(bucket_3.total_rejections, 1); // 1 (sess-1) + 0 (sess-2)
    assert!((bucket_3.mean_rejections_per_gate - 0.5).abs() < 1e-9);
    assert!((bucket_3.first_pass_rate - 0.5).abs() < 1e-9); // sess-2 only

    let bucket_5 = buckets.iter().find(|b| b.criteria_count == 5).unwrap();
    assert_eq!(bucket_5.gate_count, 2);
    assert_eq!(bucket_5.total_rejections, 3); // 0 (sess-1) + 3 (sess-2)
    assert!((bucket_5.mean_rejections_per_gate - 1.5).abs() < 1e-9);
    assert!((bucket_5.first_pass_rate - 0.5).abs() < 1e-9); // sess-1 only

    // Sorted by criteria_count ascending.
    assert_eq!(buckets[0].criteria_count, 3);
    assert_eq!(buckets[1].criteria_count, 5);

    // Mean dwell is averaged only over gates that recorded a dwell; every
    // gate here advanced, so both buckets carry a finite mean.
    assert!(bucket_3.mean_dwell_ms.is_some());
    assert!(bucket_5.mean_dwell_ms.is_some());
}
