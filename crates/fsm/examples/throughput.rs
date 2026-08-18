//! Throughput harness — drives N complete in-process enforcer sessions through
//! `ProfileFsmEngine` and reports sessions/sec and steps/sec.
//!
//! No LLM, no orchestrator, no MCP: this is the pure engine hot path. One
//! engine instance serves all N sessions (as a live gateway would), each
//! session gets a unique id and is driven to `StepOutcome::Completed` with
//! scripted, always-present checklist evidence. The per-session cost measured
//! here therefore INCLUDES the `validate_profile` paid on every
//! `start_session`, plus every `submit_milestone` transition.
//!
//! Ledger is in-memory (`MemLedger`) so the number isolates engine logic from
//! disk fsync — see `gateway_mem` (gateway crate) for the disk-backed,
//! one-engine-per-session footprint a real served session pays.
//!
//! Run:
//!   cargo run --release -p protocol-fsm --example throughput -- [N] [profile.yaml]
//! Defaults: N=1000, profile=profiles/default.yaml

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use protocol_fsm::{FsmConfig, ProfileFsmEngine, StepOutcome};
use protocol_ledger::LedgerPort;
use protocol_library::Library;
use protocol_types::{ChecklistEvidence, FsmEvent, LedgerError, Position, Profile};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repo root")
}

/// In-memory `LedgerPort` — appends are a `Vec::push`, no disk. Shared across
/// all sessions via `Arc<Mutex<..>>` (clone-per-engine is cheap).
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

/// Map every `(macro_id, sub_state_id)` to its checklist criteria (if any), off
/// the *normalized* profile (enabled-macros-only), so the driver can hand
/// exactly the evidence a checklist sub-state demands and nothing on the others.
fn criteria_index(profile: &Profile) -> HashMap<(String, String), Vec<String>> {
    let mut idx = HashMap::new();
    for m in &profile.pipeline {
        for s in &m.sub_states {
            if let Some(crit) = &s.criteria {
                idx.insert((m.state_id.clone(), s.id.clone()), crit.clone());
            }
        }
    }
    idx
}

/// Drive one session start->Completed. Returns the number of `submit_milestone`
/// steps it took.
fn drive_session(
    engine: &mut ProfileFsmEngine<MemLedger>,
    session_id: &str,
    crit_idx: &HashMap<(String, String), Vec<String>>,
) -> usize {
    let view = engine
        .start_session(session_id, None)
        .expect("start_session");
    let mut pos: Position = view.position;
    let mut steps = 0usize;

    loop {
        // Scripted evidence: present a value for every criterion of the current
        // sub-state (empty map for non-checklist subs — evidence is ignored there).
        let mut evidence = ChecklistEvidence::new();
        if let Some(crit) = crit_idx.get(&(pos.macro_id.clone(), pos.sub_state_id.clone())) {
            for c in crit {
                evidence.insert(c.clone(), serde_json::json!("done"));
            }
        }

        let outcome = engine
            .submit_milestone(session_id, pos.clone(), evidence, None)
            .expect("submit_milestone");
        steps += 1;

        match outcome {
            StepOutcome::Advanced(view) => pos = view.position,
            StepOutcome::LoopedBack { view, .. } => pos = view.position,
            StepOutcome::Completed { .. } => break,
            StepOutcome::Rejected {
                rejected_items,
                reason,
                ..
            } => panic!("unexpected rejection: {reason} items={rejected_items:?}"),
        }
    }
    steps
}

fn main() {
    let mut args = std::env::args().skip(1);
    let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1000);
    let profile_path: PathBuf = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("profiles/default.yaml"));

    let profile = protocol_manifest::load_profile(&profile_path).expect("load profile");
    let library = Library::new(repo_root().join("library"));
    let ledger = MemLedger::default();
    let mut engine = ProfileFsmEngine::new(
        Clone::clone(&profile),
        library,
        ledger,
        FsmConfig::default(),
    );
    // Build the criteria index off the SAME normalization the engine applied.
    let crit_idx = criteria_index(&profile.with_enabled_macros_only());

    // One untimed warm session so the profile-validate path / allocator are hot.
    let steps_per_session = drive_session(&mut engine, "warmup", &crit_idx);

    let start = Instant::now();
    let mut total_steps = 0usize;
    for i in 0..n {
        total_steps += drive_session(&mut engine, &format!("s{i}"), &crit_idx);
    }
    let elapsed = start.elapsed();

    let secs = elapsed.as_secs_f64();
    let sessions_per_sec = n as f64 / secs;
    let steps_per_sec = total_steps as f64 / secs;
    let us_per_session = elapsed.as_micros() as f64 / n as f64;
    let us_per_step = elapsed.as_micros() as f64 / total_steps as f64;

    println!("=== throughput harness ===");
    println!("profile:            {}", profile_path.display());
    println!("profile.name:       {}", profile.name);
    println!("sessions (N):       {n}");
    println!("steps/session:      {steps_per_session}");
    println!("total steps:        {total_steps}");
    println!("wall time:          {:.6} s", secs);
    println!("--");
    println!("sessions/sec:       {:.1}", sessions_per_sec);
    println!("steps/sec:          {:.1}", steps_per_sec);
    println!("us/session (mean):  {:.3}", us_per_session);
    println!("us/step (mean):     {:.3}", us_per_step);
}
