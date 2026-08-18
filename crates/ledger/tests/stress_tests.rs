//! Concurrency + durability stress tests for `Ledger`.
//!
//! `Ledger::append` is `&self` (the file is behind `Arc<Mutex<File>>`), so a
//! single ledger shared as `Arc<Ledger>` is written from many threads with NO
//! external lock — these tests exercise the ledger's OWN internal
//! serialization. (An earlier version wrapped the ledger in an external
//! `Arc<Mutex<Ledger>>` and locked it before every append, which serialized the
//! calls before they reached the ledger and therefore tested nothing about
//! concurrency; removing the internal lock would have left those tests green.)

use chrono::Utc;
use serde_json::json;
use std::fs;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

use protocol_ledger::Ledger;
use protocol_types::{FsmEvent, FsmEventType};

fn create_test_event(session_id: &str, iteration: u32) -> FsmEvent {
    FsmEvent {
        event_id: Uuid::new_v4(),
        timestamp: Utc::now(),
        session_id: session_id.to_string(),
        event_type: FsmEventType::MilestoneSubmitted {
            evidence_keys: vec![format!("item-{}", iteration)],
            iteration,
        },
        step_id: Some("step1".to_string()),
        payload: json!({}),
    }
}

#[test]
fn concurrent_appends_from_many_threads_are_all_persisted() {
    let temp_dir = TempDir::new().unwrap();
    // One ledger, shared. No external mutex: threads call `append(&self)`
    // concurrently and the ledger's internal `Arc<Mutex<File>>` serializes them.
    let ledger = Arc::new(Ledger::new("stress-test", temp_dir.path()).unwrap());

    let num_threads: usize = 10;
    let events_per_thread: usize = 50;
    let mut handles = vec![];

    for t in 0..num_threads {
        let ledger = ledger.clone();
        let handle = thread::spawn(move || {
            for i in 0..events_per_thread {
                let event = create_test_event("stress-test", (t * events_per_thread + i) as u32);
                ledger.append(&event).unwrap();
                if i % 10 == 0 {
                    thread::sleep(Duration::from_micros(100));
                }
            }
        });
        handles.push(handle);
    }
    for handle in handles {
        handle.join().unwrap();
    }

    let events = Ledger::replay("stress-test", temp_dir.path()).unwrap();
    assert_eq!(events.len(), num_threads * events_per_thread);
}

#[test]
fn concurrent_appends_under_high_contention_lose_no_events() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Arc::new(Ledger::new("high-contention", temp_dir.path()).unwrap());

    let num_threads: usize = 20;
    let events_per_thread: usize = 100;
    let mut handles = vec![];

    for t in 0..num_threads {
        let ledger = ledger.clone();
        let handle = thread::spawn(move || {
            for i in 0..events_per_thread {
                let event =
                    create_test_event("high-contention", (t * events_per_thread + i) as u32);
                // Tight loop, no sleeps: maximum contention on the internal lock.
                ledger.append(&event).unwrap();
            }
        });
        handles.push(handle);
    }
    for handle in handles {
        handle.join().unwrap();
    }

    let events = Ledger::replay("high-contention", temp_dir.path()).unwrap();
    assert_eq!(events.len(), num_threads * events_per_thread);

    // Every distinct iteration id appears exactly once — no torn line dropped a
    // record and no write clobbered another (which unsynchronized writes would).
    let mut iterations = std::collections::HashSet::new();
    for event in &events {
        if let FsmEventType::MilestoneSubmitted { iteration, .. } = &event.event_type {
            iterations.insert(*iteration);
        }
    }
    assert_eq!(iterations.len(), num_threads * events_per_thread);
}

#[test]
fn test_rapid_sequential_appends() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("rapid", temp_dir.path()).unwrap();

    let num_events: usize = 1000;
    for i in 0..num_events {
        let event = create_test_event("rapid", i as u32);
        ledger.append(&event).unwrap();
    }

    let events = Ledger::replay("rapid", temp_dir.path()).unwrap();
    assert_eq!(events.len(), num_events);
}

#[test]
fn replay_during_concurrent_writes_is_never_torn() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Arc::new(Ledger::new("interleaved", temp_dir.path()).unwrap());

    let num_writer_threads: usize = 5;
    let events_per_writer: usize = 20;
    let mut handles = vec![];

    for t in 0..num_writer_threads {
        let ledger = ledger.clone();
        let handle = thread::spawn(move || {
            for i in 0..events_per_writer {
                let event = create_test_event("interleaved", (t * events_per_writer + i) as u32);
                ledger.append(&event).unwrap();
            }
        });
        handles.push(handle);
    }

    // Concurrent reader: replays WHILE writers are still appending. The count is
    // nondeterministic (a snapshot somewhere between 0 and the total), so we only
    // assert the invariant that a concurrent replay never returns torn/corrupt
    // data — `replay()` errors on a malformed line, so a successful unwrap
    // already proves every line it saw was a whole, valid event.
    let reader_path = temp_dir.path().to_path_buf();
    let reader_handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        Ledger::replay("interleaved", &reader_path).unwrap()
    });

    for handle in handles {
        handle.join().unwrap();
    }

    let snapshot = reader_handle.join().unwrap();
    assert!(snapshot.len() <= num_writer_threads * events_per_writer);

    // After all writers have joined, a fresh replay must see EVERY event once.
    let replayed = Ledger::replay("interleaved", temp_dir.path()).unwrap();
    assert_eq!(replayed.len(), num_writer_threads * events_per_writer);
}

#[test]
fn test_crash_recovery_simulation() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("crash-recovery", temp_dir.path()).unwrap();

    for i in 0..100 {
        let event = create_test_event("crash-recovery", i);
        ledger.append(&event).unwrap();
    }

    let file_path = temp_dir.path().join("crash-recovery.jsonl");
    let content_before = fs::read_to_string(&file_path).unwrap();
    let lines_before: Vec<&str> = content_before.lines().collect();

    let events = Ledger::replay("crash-recovery", temp_dir.path()).unwrap();
    assert_eq!(events.len(), 100);
    assert_eq!(lines_before.len(), 100);
}

#[test]
fn test_large_payload_append() {
    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("large-payload", temp_dir.path()).unwrap();

    let large_payload = json!({
        "data": "x".repeat(10000),
        "nested": {
            "array": (0..100).collect::<Vec<_>>(),
            "object": serde_json::Map::from_iter((0..50).map(|i| (format!("key{}", i), json!(i))))
        }
    });

    let mut event = create_test_event("large-payload", 1);
    event.payload = large_payload.clone();

    ledger.append(&event).unwrap();

    let events = Ledger::replay("large-payload", temp_dir.path()).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload, large_payload);
}

#[test]
fn test_file_permissions_preserved() {
    use std::os::unix::fs::MetadataExt;

    let temp_dir = TempDir::new().unwrap();
    let ledger = Ledger::new("permissions", temp_dir.path()).unwrap();

    let event = create_test_event("permissions", 1);
    ledger.append(&event).unwrap();

    let metadata = fs::metadata(temp_dir.path().join("permissions.jsonl")).unwrap();
    let mode = metadata.mode() & 0o777;
    assert!(mode & 0o600 != 0);
}

#[test]
fn simultaneous_appends_to_different_sessions_stay_isolated() {
    let temp_dir = TempDir::new().unwrap();

    // Two independent ledgers (different files), each shared across its own
    // writer threads. Proves per-session isolation under real concurrency.
    let session1 = Arc::new(Ledger::new("session-1", temp_dir.path()).unwrap());
    let session2 = Arc::new(Ledger::new("session-2", temp_dir.path()).unwrap());

    let mut handles = vec![];
    for i in 0..50 {
        let s1 = session1.clone();
        handles.push(thread::spawn(move || {
            s1.append(&create_test_event("session-1", i)).unwrap();
        }));
        let s2 = session2.clone();
        handles.push(thread::spawn(move || {
            s2.append(&create_test_event("session-2", i)).unwrap();
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }

    let events1 = Ledger::replay("session-1", temp_dir.path()).unwrap();
    let events2 = Ledger::replay("session-2", temp_dir.path()).unwrap();
    assert_eq!(events1.len(), 50);
    assert_eq!(events2.len(), 50);
}

/// Two SEPARATE `Ledger` instances over the same on-disk file (each with its
/// own open file description) stand in for two processes sharing one
/// `PROTOCOL_LEDGER_DIR`. The intra-process `Mutex` does NOT serialize across
/// instances, so this exercises the cross-process advisory `File::lock` in
/// `append`: after both hammer the file concurrently, every line must still be
/// a well-formed event (no torn/interleaved writes) and the count must be exact.
#[test]
fn two_fds_sharing_one_file_append_without_corruption() {
    let temp_dir = TempDir::new().unwrap();
    // Two independent ledgers -> two independent fds on the same path.
    let a = Arc::new(Ledger::new("shared-file", temp_dir.path()).unwrap());
    let b = Arc::new(Ledger::new("shared-file", temp_dir.path()).unwrap());

    let per: u32 = 300;
    let mut handles = vec![];
    for (tag, ledger) in [(0u32, a.clone()), (1u32, b.clone())] {
        let handle = thread::spawn(move || {
            for i in 0..per {
                ledger
                    .append(&create_test_event("shared-file", tag * per + i))
                    .unwrap();
            }
        });
        handles.push(handle);
    }
    for handle in handles {
        handle.join().unwrap();
    }

    // `replay` hard-fails with `Corrupt` on any malformed non-trailing line, so
    // a successful replay of the exact count proves no write interleaved.
    let events = Ledger::replay("shared-file", temp_dir.path()).unwrap();
    assert_eq!(
        events.len() as u32,
        per * 2,
        "every append from both fds must be persisted intact"
    );
}
