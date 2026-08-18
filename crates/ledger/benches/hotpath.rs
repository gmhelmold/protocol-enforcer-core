// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Hot-path performance benches for the ledger crate.
//!
//! Two things the enforcer pays for at runtime live here:
//!   (c) `Ledger::append` — one event written to the on-disk JSONL ledger
//!       (serialize + write_all + flush + fdatasync, under an advisory lock).
//!   (d) `merkle::root` — the RFC-6962 transcript root over N leaves, computed
//!       ONCE per session completion. Swept at N = 1, 10, 100, 1000, 10000 so
//!       the scaling of the root cost is a measured number, not a claim.
//!
//! `harness = false` in Cargo.toml — criterion owns `main`.

use chrono::Utc;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use protocol_ledger::merkle;
use protocol_ledger::Ledger;
use protocol_types::{FsmEvent, FsmEventType};
use uuid::Uuid;

/// A realistic ledger leaf: a serialized `SubStateAdvanced` line is ~180-260
/// bytes on the wire. We build leaves of that size so the SHA-256 leaf-hash cost
/// reflects real transcript lines, not empty buffers.
fn realistic_leaves(n: usize) -> Vec<Vec<u8>> {
    (0..n)
        .map(|i| {
            format!(
                "{{\"event_id\":\"{:032x}\",\"timestamp\":\"2026-08-17T00:00:00Z\",\
                 \"session_id\":\"bench-session-01\",\"event_type\":{{\"type\":\
                 \"SubStateAdvanced\",\"from_sub\":\"reproduce\",\"to_sub\":\
                 \"root_cause\"}},\"step_id\":\"understand/reproduce\",\"payload\":{{}}}}",
                i
            )
            .into_bytes()
        })
        .collect()
}

/// One representative event to append (a plain sub-state transition — the most
/// common line the enforcer writes).
fn sample_event(i: u64) -> FsmEvent {
    FsmEvent {
        event_id: Uuid::from_u128(i as u128),
        timestamp: Utc::now(),
        session_id: "bench-session-01".to_string(),
        event_type: FsmEventType::SubStateAdvanced {
            from_sub: "reproduce".to_string(),
            to_sub: "root_cause".to_string(),
        },
        step_id: Some("understand/reproduce".to_string()),
        payload: serde_json::json!({}),
    }
}

/// (d) RFC-6962 transcript root as a function of tree size.
fn bench_transcript_root(c: &mut Criterion) {
    let mut group = c.benchmark_group("transcript_root_rfc6962");
    for &n in &[1usize, 10, 100, 1000, 10000] {
        let leaves = realistic_leaves(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &leaves, |b, leaves| {
            b.iter(|| {
                let r = merkle::root(black_box(leaves));
                black_box(r);
            });
        });
    }
    group.finish();
}

/// (c) One event appended to a real on-disk ledger (serialize + write + fsync).
/// The ledger file grows across iterations; append is O(1) in file size (it is a
/// pure append + fdatasync), so a growing file does not bias the per-op number.
fn bench_ledger_append(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = Ledger::new("bench-session-01", dir.path()).expect("ledger");
    let mut i = 0u64;
    c.bench_function("ledger_append_one_event_fsync", |b| {
        b.iter(|| {
            i += 1;
            ledger.append(black_box(&sample_event(i))).expect("append");
        });
    });
}

criterion_group!(benches, bench_transcript_root, bench_ledger_append);
criterion_main!(benches);
