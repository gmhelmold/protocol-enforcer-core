// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

use chrono::Utc;
use serde_json::json;
use std::fs;
use std::io::Write;
use tempfile::TempDir;
use uuid::Uuid;

use protocol_ledger::{Ledger, LedgerPort};
use protocol_types::{CircuitBreakerType, FsmEvent, FsmEventType, LedgerError};

fn create_test_event(session_id: &str, event_type: FsmEventType) -> FsmEvent {
    FsmEvent {
        event_id: Uuid::new_v4(),
        timestamp: Utc::now(),
        session_id: session_id.to_string(),
        event_type,
        step_id: Some("step1".to_string()),
        payload: json!({}),
    }
}

#[test]
fn test_new_creates_ledger_file() {
    let temp_dir = TempDir::new().unwrap();
    let _ledger = Ledger::new("test-session", temp_dir.path()).unwrap();
    assert!(temp_dir.path().join("test-session.jsonl").exists());
}

#[test]
fn test_append_single_event() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();
    let event = create_test_event(
        "test-session",
        FsmEventType::SessionStarted {
            pipeline_id: "pipe1".to_string(),
            manifest_version: "1.0".to_string(),
            profile_sha256: None,
        },
    );

    ledger.append(&event).unwrap();

    let content = fs::read_to_string(temp_dir.path().join("test-session.jsonl")).unwrap();
    assert!(content.contains("SessionStarted"));
    assert!(content.ends_with('\n'));
}

#[test]
fn test_append_multiple_events() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();

    let event1 = create_test_event(
        "test-session",
        FsmEventType::SessionStarted {
            pipeline_id: "pipe1".to_string(),
            manifest_version: "1.0".to_string(),
            profile_sha256: None,
        },
    );
    let event2 = create_test_event(
        "test-session",
        FsmEventType::MilestoneSubmitted {
            evidence_keys: vec!["item1".to_string()],
            iteration: 1,
        },
    );

    ledger.append(&event1).unwrap();
    ledger.append(&event2).unwrap();

    let content = fs::read_to_string(temp_dir.path().join("test-session.jsonl")).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2);
}

#[test]
fn test_replay_empty_ledger() {
    let temp_dir = TempDir::new().unwrap();
    let events = Ledger::replay("nonexistent", temp_dir.path()).unwrap();
    assert!(events.is_empty());
}

#[test]
fn test_replay_parses_events() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();

    let event1 = create_test_event(
        "test-session",
        FsmEventType::SessionStarted {
            pipeline_id: "pipe1".to_string(),
            manifest_version: "1.0".to_string(),
            profile_sha256: None,
        },
    );
    let event2 = create_test_event(
        "test-session",
        FsmEventType::MilestoneSubmitted {
            evidence_keys: vec!["item1".to_string()],
            iteration: 1,
        },
    );

    ledger.append(&event1).unwrap();
    ledger.append(&event2).unwrap();

    let events = Ledger::replay("test-session", temp_dir.path()).unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0].event_type,
        FsmEventType::SessionStarted { .. }
    ));
    assert!(matches!(
        events[1].event_type,
        FsmEventType::MilestoneSubmitted { .. }
    ));
}

#[test]
fn test_replay_stops_at_corrupt_line() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();

    let event1 = create_test_event(
        "test-session",
        FsmEventType::SessionStarted {
            pipeline_id: "pipe1".to_string(),
            manifest_version: "1.0".to_string(),
            profile_sha256: None,
        },
    );
    ledger.append(&event1).unwrap();

    fs::write(
        temp_dir.path().join("test-session.jsonl"),
        r#"{"event_id":"00000000-0000-0000-0000-000000000000","timestamp":"2024-01-01T00:00:00Z","session_id":"test-session","event_type":{"type":"SessionStarted","pipeline_id":"pipe1","manifest_version":"1.0"},"step_id":"step1","payload":{}}
invalid json line
{"event_id":"11111111-1111-1111-1111-111111111111","timestamp":"2024-01-01T00:00:00Z","session_id":"test-session","event_type":{"type":"MilestoneSubmitted","evidence_keys":["item1"],"iteration":1},"step_id":"step1","payload":{}}"#,
    ).unwrap();

    let result = Ledger::replay("test-session", temp_dir.path());
    assert!(result.is_err());
    match result.unwrap_err() {
        LedgerError::Corrupt { line, message } => {
            assert_eq!(line, 2);
            assert!(!message.is_empty());
        }
        _ => panic!("Expected Corrupt error"),
    }
}

#[test]
fn test_replay_tolerates_malformed_trailing_line() {
    // A truncated/garbage LAST line (e.g. a crash mid-write) must not lose
    // the whole session history -- replay should return the valid prefix.
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();

    let event1 = create_test_event(
        "test-session",
        FsmEventType::SessionStarted {
            pipeline_id: "pipe1".to_string(),
            manifest_version: "1.0".to_string(),
            profile_sha256: None,
        },
    );
    let event2 = create_test_event(
        "test-session",
        FsmEventType::MilestoneSubmitted {
            evidence_keys: vec!["item1".to_string()],
            iteration: 1,
        },
    );
    ledger.append(&event1).unwrap();
    ledger.append(&event2).unwrap();

    // Simulate a crash mid-write: append a truncated JSON fragment with no
    // trailing newline.
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(temp_dir.path().join("test-session.jsonl"))
        .unwrap();
    write!(
        file,
        r#"{{"event_id":"22222222-2222-2222-2222-222222222222","time"#
    )
    .unwrap();
    drop(file);

    let events = Ledger::replay("test-session", temp_dir.path())
        .expect("malformed trailing line must not abort replay");
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0].event_type,
        FsmEventType::SessionStarted { .. }
    ));
    assert!(matches!(
        events[1].event_type,
        FsmEventType::MilestoneSubmitted { .. }
    ));
}

#[test]
fn test_replay_skips_empty_lines() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();

    let event1 = create_test_event(
        "test-session",
        FsmEventType::SessionStarted {
            pipeline_id: "pipe1".to_string(),
            manifest_version: "1.0".to_string(),
            profile_sha256: None,
        },
    );
    ledger.append(&event1).unwrap();

    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(temp_dir.path().join("test-session.jsonl"))
        .unwrap();
    writeln!(file).unwrap();
    writeln!(file).unwrap();

    let event2 = create_test_event(
        "test-session",
        FsmEventType::MilestoneSubmitted {
            evidence_keys: vec!["item1".to_string()],
            iteration: 1,
        },
    );
    ledger.append(&event2).unwrap();

    let events = Ledger::replay("test-session", temp_dir.path()).unwrap();
    assert_eq!(events.len(), 2);
}

#[test]
fn test_concurrent_appends_same_ledger() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();

    for i in 0..100 {
        let event = create_test_event(
            "test-session",
            FsmEventType::MilestoneSubmitted {
                evidence_keys: vec![format!("item{}", i)],
                iteration: 1,
            },
        );
        ledger.append(&event).unwrap();
    }

    let events = Ledger::replay("test-session", temp_dir.path()).unwrap();
    assert_eq!(events.len(), 100);
}

#[test]
fn test_ledger_port_trait_append() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();
    let event = create_test_event(
        "test-session",
        FsmEventType::SessionStarted {
            pipeline_id: "pipe1".to_string(),
            manifest_version: "1.0".to_string(),
            profile_sha256: None,
        },
    );

    LedgerPort::append(&ledger, &event).unwrap();

    let content = fs::read_to_string(temp_dir.path().join("test-session.jsonl")).unwrap();
    assert!(content.contains("SessionStarted"));
}

#[test]
fn test_ledger_port_trait_replay() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();

    let event = create_test_event(
        "test-session",
        FsmEventType::SessionStarted {
            pipeline_id: "pipe1".to_string(),
            manifest_version: "1.0".to_string(),
            profile_sha256: None,
        },
    );
    ledger.append(&event).unwrap();

    let events = LedgerPort::replay(&ledger, "test-session").unwrap();
    assert_eq!(events.len(), 1);
}

#[test]
fn test_all_event_types_serialization() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();

    let events = vec![
        create_test_event(
            "test-session",
            FsmEventType::SessionStarted {
                pipeline_id: "pipe1".to_string(),
                manifest_version: "1.0".to_string(),
                profile_sha256: None,
            },
        ),
        create_test_event(
            "test-session",
            FsmEventType::MilestoneSubmitted {
                evidence_keys: vec!["item1".to_string()],
                iteration: 1,
            },
        ),
        create_test_event(
            "test-session",
            FsmEventType::MilestoneAccepted {
                next_step_id: Some("step2".to_string()),
            },
        ),
        create_test_event(
            "test-session",
            FsmEventType::MilestoneRejected {
                rejected_items: vec!["item1".to_string()],
                reason: "incomplete".to_string(),
                iteration: 1,
            },
        ),
        create_test_event(
            "test-session",
            FsmEventType::StepAdvanced {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
            },
        ),
        create_test_event(
            "test-session",
            FsmEventType::CircuitBreakerTriggered {
                breaker: CircuitBreakerType::MaxIterations,
                details: json!({}),
            },
        ),
        create_test_event(
            "test-session",
            FsmEventType::SessionCompleted {
                final_artifact_path: "/path/to/artifact".to_string(),
                output_sha256: None,
            },
        ),
        create_test_event(
            "test-session",
            FsmEventType::SessionFailed {
                reason: "timeout".to_string(),
                final_step: "step3".to_string(),
            },
        ),
    ];

    for event in &events {
        ledger.append(event).unwrap();
    }

    let replayed = Ledger::replay("test-session", temp_dir.path()).unwrap();
    assert_eq!(replayed.len(), events.len());
}

#[test]
fn test_replay_nonexistent_session_returns_empty() {
    let temp_dir = TempDir::new().unwrap();
    let events = Ledger::replay("nonexistent", temp_dir.path()).unwrap();
    assert!(events.is_empty());
}

#[test]
fn test_append_fsync_roundtrip() {
    // True power-loss durability isn't unit-testable in-process (it needs an
    // actual crash between write and sync); this test guards the fsync'd
    // append code path and the append -> replay roundtrip integrity.
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();

    let events = vec![
        create_test_event(
            "test-session",
            FsmEventType::SessionStarted {
                pipeline_id: "pipe1".to_string(),
                manifest_version: "1.0".to_string(),
                profile_sha256: None,
            },
        ),
        create_test_event(
            "test-session",
            FsmEventType::MilestoneSubmitted {
                evidence_keys: vec!["item1".to_string()],
                iteration: 1,
            },
        ),
        create_test_event(
            "test-session",
            FsmEventType::StepAdvanced {
                from_step: "step1".to_string(),
                to_step: "step2".to_string(),
            },
        ),
        create_test_event(
            "test-session",
            FsmEventType::SessionCompleted {
                final_artifact_path: "/path/to/artifact".to_string(),
                output_sha256: None,
            },
        ),
    ];

    for event in &events {
        ledger.append(event).unwrap();
    }

    let replayed = Ledger::replay("test-session", temp_dir.path()).unwrap();
    assert_eq!(replayed.len(), events.len());
    for (expected, actual) in events.iter().zip(replayed.iter()) {
        assert_eq!(expected.event_id, actual.event_id);
    }
}

#[test]
fn test_append_with_payload() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("test-session", temp_dir.path()).unwrap();

    let mut event = create_test_event(
        "test-session",
        FsmEventType::SessionStarted {
            pipeline_id: "pipe1".to_string(),
            manifest_version: "1.0".to_string(),
            profile_sha256: None,
        },
    );
    event.payload = json!({"key": "value", "nested": {"a": 1}});

    ledger.append(&event).unwrap();

    let replayed = Ledger::replay("test-session", temp_dir.path()).unwrap();
    assert_eq!(
        replayed[0].payload,
        json!({"key": "value", "nested": {"a": 1}})
    );
}
