//! SPEC_loopback.md metrics acceptance tests (#6 `metrics_looped`, #10
//! `loop_then_fail_metrics_passed_false`), split out of `metrics_tests.rs`
//! to keep that file under the 600-line cap (`scripts/check_loc.sh`).
//! Deterministic: explicit `DateTime` timestamps only, never `Utc::now()`.

use chrono::DateTime;
use protocol_ledger::metrics::compute_session_metrics;
use protocol_types::{FsmEvent, FsmEventType};
use serde_json::json;
use uuid::Uuid;

/// Build an event with an explicit deterministic timestamp
/// (`"2026-01-01T00:00:0{offset_secs}Z"`). Mirrors `metrics_tests.rs::ev`.
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

/// A session with one macro loop-back (SPEC_loopback.md): `draft/s1`
/// (execute, the loop target) is visited twice, `draft/chk1` is rejected
/// once then looped, then re-visited and passes clean.
fn looped_then_passed_session(session_id: &str) -> Vec<FsmEvent> {
    vec![
        ev(
            session_id,
            0,
            None,
            FsmEventType::SessionStarted {
                pipeline_id: "p".to_string(),
                manifest_version: "1".to_string(),
                profile_sha256: None,
            },
        ),
        // Arrival at s1 (1st visit).
        ev(
            session_id,
            1,
            Some("draft/s1"),
            FsmEventType::SubStateAdvanced {
                from_sub: "s0".to_string(),
                to_sub: "s1".to_string(),
            },
        ),
        // Arrival at chk1 (leaves s1's 1st visit: dwell 1s -> 4s = 3000ms).
        ev(
            session_id,
            4,
            Some("draft/chk1"),
            FsmEventType::SubStateAdvanced {
                from_sub: "s1".to_string(),
                to_sub: "chk1".to_string(),
            },
        ),
        ev(
            session_id,
            5,
            Some("draft/chk1"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["review_ack".to_string()],
                reason: "checklist_incomplete".to_string(),
                iteration: 1,
            },
        ),
        // MacroLoopedBack anchored at s1 (arrival = 2nd visit); leaves
        // chk1's 1st run (dwell 4s -> 6s = 2000ms).
        ev(
            session_id,
            6,
            Some("draft/s1"),
            FsmEventType::MacroLoopedBack {
                from_sub: "chk1".to_string(),
                to_sub: "s1".to_string(),
                iteration: 1,
            },
        ),
        // Leaves s1's 2nd visit (dwell 6s -> 9s = 3000ms); total s1 dwell =
        // 3000 + 3000 = 6000ms across both visits.
        ev(
            session_id,
            9,
            Some("draft/chk1"),
            FsmEventType::SubStateAdvanced {
                from_sub: "s1".to_string(),
                to_sub: "chk1".to_string(),
            },
        ),
        ev(
            session_id,
            10,
            Some("draft/chk1"),
            FsmEventType::MilestoneAccepted { next_step_id: None },
        ),
        ev(
            session_id,
            11,
            None,
            FsmEventType::MacroAdvanced {
                from_macro: "draft".to_string(),
                to_macro: "ship".to_string(),
            },
        ),
        ev(
            session_id,
            12,
            None,
            FsmEventType::SessionCompleted {
                final_artifact_path: "/tmp/out.json".to_string(),
                output_sha256: None,
            },
        ),
    ]
}

/// #6 metrics_looped: the execute step's dwell_ms sums both visits; the
/// checklist step's rejections == 1; nothing mis-attributed (in particular,
/// the loop round's MacroLoopedBack must not itself count as a pass for
/// chk1 -- it passes only via the later MilestoneAccepted).
#[test]
fn metrics_looped() {
    let events = looped_then_passed_session("sess-looped");
    let metrics = compute_session_metrics(&events, None);

    let s1 = metrics
        .steps
        .iter()
        .find(|s| s.sub_state_id == "s1")
        .expect("s1 present");
    assert_eq!(
        s1.dwell_ms,
        Some(6000),
        "s1's dwell sums both visits (3000 + 3000)"
    );
    // s1 is an execute sub-state: it never records a MilestoneAccepted
    // itself (only Checklist steps do), so `passed` stays false for it even
    // though the session as a whole moved past it twice -- `passed` is a
    // checklist-shaped predicate, dwell is what attributes across visits.
    assert!(!s1.passed);

    let chk1 = metrics
        .steps
        .iter()
        .find(|s| s.sub_state_id == "chk1")
        .expect("chk1 present");
    assert_eq!(chk1.rejections, 1);
    assert!(chk1.passed, "chk1 passed via the later MilestoneAccepted");
    // chk1's dwell sums across BOTH its runs: the loop round (4s->6s =
    // 2000ms, ending in MacroLoopedBack) and the final pass round (9s->10s =
    // 1000ms, ending in MilestoneAccepted) = 3000ms.
    assert_eq!(chk1.dwell_ms, Some(3000));
}

/// #10 loop_then_fail_metrics_passed_false: a session that loops once then
/// trips MaxIterations -> the checklist step must report passed:false
/// (guards the metrics-`passed` conflation: a naive single-`find` would let
/// `MacroLoopedBack` satisfy "did it advance?" and wrongly report
/// passed:true after only a loop, never an actual accept). The execute
/// step's dwell_ms still sums both visits.
#[test]
fn loop_then_fail_metrics_passed_false() {
    let session_id = "sess-loop-then-fail";
    let events = vec![
        ev(
            session_id,
            0,
            None,
            FsmEventType::SessionStarted {
                pipeline_id: "p".to_string(),
                manifest_version: "1".to_string(),
                profile_sha256: None,
            },
        ),
        ev(
            session_id,
            1,
            Some("draft/s1"),
            FsmEventType::SubStateAdvanced {
                from_sub: "s0".to_string(),
                to_sub: "s1".to_string(),
            },
        ),
        ev(
            session_id,
            2,
            Some("draft/chk1"),
            FsmEventType::SubStateAdvanced {
                from_sub: "s1".to_string(),
                to_sub: "chk1".to_string(),
            },
        ),
        ev(
            session_id,
            3,
            Some("draft/chk1"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["review_ack".to_string()],
                reason: "checklist_incomplete".to_string(),
                iteration: 1,
            },
        ),
        // 1st rejection loops back (2nd visit to s1 begins here).
        ev(
            session_id,
            4,
            Some("draft/s1"),
            FsmEventType::MacroLoopedBack {
                from_sub: "chk1".to_string(),
                to_sub: "s1".to_string(),
                iteration: 1,
            },
        ),
        ev(
            session_id,
            6,
            Some("draft/chk1"),
            FsmEventType::SubStateAdvanced {
                from_sub: "s1".to_string(),
                to_sub: "chk1".to_string(),
            },
        ),
        // 2nd rejection trips MaxIterations -- session fails, chk1 never
        // passes.
        ev(
            session_id,
            7,
            Some("draft/chk1"),
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["review_ack".to_string()],
                reason: "checklist_incomplete".to_string(),
                iteration: 2,
            },
        ),
        ev(
            session_id,
            7,
            Some("draft/chk1"),
            FsmEventType::CircuitBreakerTriggered {
                breaker: protocol_types::CircuitBreakerType::MaxIterations,
                details: json!({}),
            },
        ),
    ];

    let metrics = compute_session_metrics(&events, None);
    assert_eq!(metrics.status, "failed");

    let chk1 = metrics
        .steps
        .iter()
        .find(|s| s.sub_state_id == "chk1")
        .expect("chk1 present");
    assert!(
        !chk1.passed,
        "a loop-back must never itself satisfy the passed predicate"
    );
    assert_eq!(chk1.rejections, 2);

    let s1 = metrics
        .steps
        .iter()
        .find(|s| s.sub_state_id == "s1")
        .expect("s1 present");
    // 1st visit: 1s -> 2s = 1000ms. 2nd visit: 4s -> 6s = 2000ms. Sum: 3000ms.
    assert_eq!(s1.dwell_ms, Some(3000));
}
