//! Hot-path performance benches for the enforcer engine.
//!
//!   (a) profile load + validate — `load_profile` (YAML read+parse) followed by
//!       `validate_profile`. This is paid ONCE per `start_session`, not per step.
//!   (b) one FSM step — a single `submit_milestone` plain sub-state transition,
//!       the actual per-step cost the agent pays. Measured two ways:
//!         * `mem_ledger`  — pure engine logic (in-memory `LedgerPort`),
//!         * `disk_ledger` — same step against a real on-disk ledger (append +
//!                           fsync included), i.e. what a real session pays.
//!
//! `harness = false` in Cargo.toml — criterion owns `main`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use protocol_fsm::{FsmConfig, ProfileFsmEngine};
use protocol_ledger::{Ledger, LedgerPort};
use protocol_library::Library;
use protocol_types::{ChecklistEvidence, FsmEvent, LedgerError, Position, Profile};

fn repo_root() -> PathBuf {
    // <repo>/crates/fsm -> <repo>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repo root")
}

fn profile_path() -> PathBuf {
    repo_root().join("profiles/quick-bug-fix.yaml")
}

fn library() -> Library {
    Library::new(repo_root().join("library"))
}

fn load_profile() -> Profile {
    protocol_manifest::load_profile(&profile_path()).expect("load quick-bug-fix.yaml")
}

/// In-memory `LedgerPort` so bench (b) can isolate the engine's transition logic
/// from disk I/O.
#[derive(Clone, Default)]
struct MemLedger(Arc<Mutex<Vec<FsmEvent>>>);

impl LedgerPort for MemLedger {
    fn append(&self, event: &FsmEvent) -> Result<(), LedgerError> {
        self.0.lock().unwrap().push(event.clone());
        Ok(())
    }
    fn replay(&self, session_id: &str) -> Result<Vec<FsmEvent>, LedgerError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.session_id == session_id)
            .cloned()
            .collect())
    }
}

/// (a) load YAML profile + validate it against the library.
fn bench_profile_load_validate(c: &mut Criterion) {
    let path = profile_path();
    let lib = library();
    c.bench_function("profile_load_and_validate", |b| {
        b.iter(|| {
            let profile = protocol_manifest::load_profile(black_box(&path)).expect("load profile");
            protocol_manifest::validate_profile(black_box(&profile), black_box(&lib))
                .expect("valid profile");
            black_box(profile);
        });
    });
}

/// Just the validate half, on an already-parsed profile (so the number is not
/// dominated by the YAML disk read + parse in (a)).
fn bench_profile_validate_only(c: &mut Criterion) {
    let profile = load_profile();
    let lib = library();
    c.bench_function("profile_validate_only", |b| {
        b.iter(|| {
            protocol_manifest::validate_profile(black_box(&profile), black_box(&lib))
                .expect("valid profile");
        });
    });
}

/// The position of quick-bug-fix's first sub-state (`understand/inject`, a plain
/// `inject` sub-state — a single non-checklist advance).
fn first_position() -> Position {
    Position {
        macro_id: "understand".to_string(),
        sub_state_id: "inject".to_string(),
    }
}

/// (b) one plain `submit_milestone` transition against an in-memory ledger.
/// `iter_batched` builds a fresh, started engine per iteration in setup (NOT
/// timed) and times exactly one `submit_milestone`.
fn bench_fsm_step_mem(c: &mut Criterion) {
    let profile = load_profile();
    let lib = library();
    c.bench_function("fsm_step_mem_ledger", |b| {
        b.iter_batched(
            || {
                let mut engine = ProfileFsmEngine::new(
                    Clone::clone(&profile),
                    lib.clone(),
                    MemLedger::default(),
                    FsmConfig::default(),
                );
                engine.start_session("bench", None).expect("start");
                engine
            },
            |mut engine| {
                let out = engine
                    .submit_milestone("bench", first_position(), ChecklistEvidence::new(), None)
                    .expect("advance");
                black_box(out);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

/// (b') the same single transition, but against a real on-disk ledger — the
/// append + fdatasync a live session actually pays each step.
fn bench_fsm_step_disk(c: &mut Criterion) {
    let profile = load_profile();
    let lib = library();
    c.bench_function("fsm_step_disk_ledger_fsync", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().expect("tempdir");
                let ledger = Ledger::new("bench", dir.path()).expect("ledger");
                let mut engine = ProfileFsmEngine::new(
                    Clone::clone(&profile),
                    lib.clone(),
                    ledger,
                    FsmConfig::default(),
                );
                engine.start_session("bench", None).expect("start");
                // keep `dir` alive for the duration of the timed step
                (engine, dir)
            },
            |(mut engine, _dir)| {
                let out = engine
                    .submit_milestone("bench", first_position(), ChecklistEvidence::new(), None)
                    .expect("advance");
                black_box(out);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_profile_load_validate,
    bench_profile_validate_only,
    bench_fsm_step_mem,
    bench_fsm_step_disk
);
criterion_main!(benches);
