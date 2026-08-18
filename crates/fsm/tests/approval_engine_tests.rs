//! Engine-level acceptance for the `human_approval` gate
//! (SPEC_human_approval.md), driven through the real `ProfileFsmEngine` against
//! a real on-disk ledger — the same shape `recovery.rs`'s in-crate test uses.
//!
//! What each test pins down:
//! - the gate issues a challenge on entry and opens for a correctly-signed submit;
//! - a signature from any OTHER key is refused (the gate is the key, not the ritual);
//! - a signature bound to a different session/macro/sub-state is refused, so a
//!   signature harvested at one gate cannot be spent at another;
//! - re-entering the gate mints a new challenge that KILLS the old signature;
//! - recovery replays the SAME challenge out of the ledger, so a crash does not
//!   silently invalidate an approval a human already gave.

use protocol_fsm::{FsmConfig, ProfileFsmEngine, StepOutcome, StepView};
use protocol_library::Library;
use protocol_types::{
    ChecklistEvidence, CircuitBreakerType, FsmError, Position, Profile, ProfileSettings,
    SessionStatus, StateDef, SubStateDef, SubStateType,
};
use std::collections::HashMap;
use std::path::Path;

/// The approver's key. Published and worthless — it exists only so this test
/// can play the human.
const SEED: &str = "0707070707070707070707070707070707070707070707070707070707070707";
/// A key the approver does NOT hold.
const IMPOSTOR_SEED: &str = "0909090909090909090909090909090909090909090909090909090909090909";

const SESSION: &str = "sess-approval";

fn pubkey() -> String {
    protocol_notary::pubkey_hex_from_seed_hex(SEED).expect("demo seed parses")
}

fn sign(seed: &str, session: &str, macro_id: &str, sub: &str, challenge: &str) -> String {
    protocol_notary::sign_approval_hex(seed, session, macro_id, sub, challenge).expect("sign")
}

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

fn gate(id: &str) -> SubStateDef {
    SubStateDef {
        approver_pubkey: Some(pubkey()),
        approval_prompt: Some("Please review and sign.".to_string()),
        ..sub_state(id, SubStateType::HumanApproval)
    }
}

fn macro_state(state_id: &str, loop_state: bool, sub_states: Vec<SubStateDef>) -> StateDef {
    StateDef {
        state_id: state_id.to_string(),
        name: state_id.to_string(),
        description: String::new(),
        system_prompt: None,
        enabled: true,
        max_iterations: 9,
        loop_state,
        icon: None,
        sub_states,
        hooks: Vec::new(),
    }
}

/// `approve` (work → gate → checklist, loop-back enabled) then a trailing
/// `ship` macro, since loop-back is forbidden on the final macro.
fn profile() -> Profile {
    Profile {
        name: "approval-test".to_string(),
        version: "1.0.0".to_string(),
        description: String::new(),
        protected: false,
        created_at: String::new(),
        cloned_from: None,
        settings: ProfileSettings::default(),
        output_contract: None,
        pipeline: vec![
            macro_state(
                "approve",
                true,
                vec![
                    sub_state("work", SubStateType::Execute),
                    gate("human_gate"),
                    checklist("chk", "approved"),
                ],
            ),
            macro_state("ship", false, vec![checklist("chk2", "shipped")]),
        ],
    }
}

fn engine_on(dir: &Path) -> ProfileFsmEngine<protocol_ledger::Ledger> {
    let library = Library::new(dir.join("library"));
    let ledger = protocol_ledger::Ledger::new(SESSION, dir).expect("ledger");
    ProfileFsmEngine::new(profile(), library, ledger, FsmConfig::default())
}

/// Same profile, but with a small `approval_rejection_cap` so the FIX 3
/// (issue #70) breaker can be reached in a handful of submissions.
fn engine_on_with_cap(dir: &Path, cap: u32) -> ProfileFsmEngine<protocol_ledger::Ledger> {
    let mut p = profile();
    p.settings.approval_rejection_cap = cap;
    let library = Library::new(dir.join("library"));
    let ledger = protocol_ledger::Ledger::new(SESSION, dir).expect("ledger");
    ProfileFsmEngine::new(p, library, ledger, FsmConfig::default())
}

fn evidence(pairs: &[(&str, &str)]) -> ChecklistEvidence {
    let mut m: ChecklistEvidence = HashMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), serde_json::json!(v));
    }
    m
}

fn advanced(outcome: StepOutcome) -> StepView {
    match outcome {
        StepOutcome::Advanced(view) => view,
        other => panic!("expected Advanced, got {other:?}"),
    }
}

/// Start a session and step off `work`, landing on the gate. Returns the
/// gate's step view (whose `injected` carries the freshly issued challenge).
fn drive_to_gate(engine: &mut ProfileFsmEngine<protocol_ledger::Ledger>) -> StepView {
    let start = engine.start_session(SESSION, None).expect("start");
    assert_eq!(start.position.sub_state_id, "work");
    assert!(
        start.injected.approval_challenge.is_none(),
        "a non-gate sub-state must not carry a challenge"
    );
    advanced(
        engine
            .submit_milestone(SESSION, start.position, evidence(&[]), None)
            .expect("advance off work"),
    )
}

fn gate_position() -> Position {
    Position {
        macro_id: "approve".to_string(),
        sub_state_id: "human_gate".to_string(),
    }
}

/// Submit `sig` as the gate's `approval_signature`. A submission is always an
/// outcome (accept or refuse), never an `Err` — that distinction is the point.
fn submit_sig(engine: &mut ProfileFsmEngine<protocol_ledger::Ledger>, sig: &str) -> StepOutcome {
    engine
        .submit_milestone(
            SESSION,
            gate_position(),
            evidence(&[("approval_signature", sig)]),
            None,
        )
        .expect("a submission is an outcome, not an error")
}

fn reject_reason(outcome: StepOutcome) -> String {
    match outcome {
        StepOutcome::Rejected {
            reason,
            rejected_items,
            position,
        } => {
            assert_eq!(rejected_items, vec!["approval_signature".to_string()]);
            assert_eq!(
                position,
                gate_position(),
                "a refusal must not move the session"
            );
            reason
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[test]
fn entering_the_gate_issues_a_challenge_and_a_signed_submit_opens_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut engine = engine_on(tmp.path());
    let at_gate = drive_to_gate(&mut engine);

    assert_eq!(at_gate.position, gate_position());
    let challenge = at_gate
        .injected
        .approval_challenge
        .clone()
        .expect("the gate must carry a challenge");
    assert_eq!(challenge.len(), 64, "32 random bytes in hex");
    assert_eq!(
        at_gate.injected.approval_prompt.as_deref(),
        Some("Please review and sign."),
        "the human's prompt must reach the agent that has to relay it"
    );

    let sig = sign(SEED, SESSION, "approve", "human_gate", &challenge);
    let next = advanced(submit_sig(&mut engine, &sig));
    assert_eq!(next.position.sub_state_id, "chk");

    // The approval is in the ledger, signature and all, so an auditor can
    // re-verify it offline without this engine.
    let events = protocol_ledger::Ledger::replay(SESSION, tmp.path()).expect("replay");
    let accepted = events
        .iter()
        .find_map(|e| match &e.event_type {
            protocol_types::FsmEventType::ApprovalAccepted {
                challenge,
                signature,
                ..
            } => Some((challenge.clone(), signature.clone())),
            _ => None,
        })
        .expect("an ApprovalAccepted event");
    assert_eq!(accepted.0, challenge);
    assert!(protocol_notary::verify_approval_hex(
        &pubkey(),
        SESSION,
        "approve",
        "human_gate",
        &accepted.0,
        &accepted.1
    ));
}

#[test]
fn an_unsigned_submit_is_rejected_as_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut engine = engine_on(tmp.path());
    drive_to_gate(&mut engine);

    let outcome = engine
        .submit_milestone(SESSION, gate_position(), evidence(&[]), None)
        .expect("a refusal is an outcome, not an error");
    assert_eq!(reject_reason(outcome), "approval_missing");
}

#[test]
fn a_signature_from_another_key_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let mut engine = engine_on(tmp.path());
    let challenge = drive_to_gate(&mut engine)
        .injected
        .approval_challenge
        .expect("challenge");

    // Correctly formed, correctly bound — just not the approver's key.
    let sig = sign(IMPOSTOR_SEED, SESSION, "approve", "human_gate", &challenge);
    let outcome = submit_sig(&mut engine, &sig);
    assert_eq!(reject_reason(outcome), "approval_invalid");
}

#[test]
fn a_signature_bound_to_a_different_gate_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let mut engine = engine_on(tmp.path());
    let challenge = drive_to_gate(&mut engine)
        .injected
        .approval_challenge
        .expect("challenge");

    // The RIGHT key over the RIGHT challenge, but bound to another session,
    // another macro, or another sub-state. Each must be worthless here.
    let mis_bound = [
        sign(
            SEED,
            "some-other-session",
            "approve",
            "human_gate",
            &challenge,
        ),
        sign(SEED, SESSION, "other_macro", "human_gate", &challenge),
        sign(SEED, SESSION, "approve", "other_gate", &challenge),
    ];
    for sig in mis_bound {
        let outcome = submit_sig(&mut engine, &sig);
        assert_eq!(reject_reason(outcome), "approval_invalid");
    }
}

#[test]
fn a_reissued_challenge_retires_the_previous_signature() {
    let tmp = tempfile::tempdir().unwrap();
    let mut engine = engine_on(tmp.path());
    let first_challenge = drive_to_gate(&mut engine)
        .injected
        .approval_challenge
        .expect("challenge");
    let stale_sig = sign(SEED, SESSION, "approve", "human_gate", &first_challenge);

    // Pass the gate, then fail the macro's checklist so the session loops back
    // to `work` and walks into the gate a SECOND time.
    advanced(submit_sig(&mut engine, &stale_sig));
    let looped = match engine
        .submit_milestone(
            SESSION,
            Position {
                macro_id: "approve".to_string(),
                sub_state_id: "chk".to_string(),
            },
            evidence(&[]),
            None,
        )
        .expect("checklist rejection")
    {
        StepOutcome::LoopedBack { view, .. } => view,
        other => panic!("expected LoopedBack, got {other:?}"),
    };
    assert_eq!(looped.position.sub_state_id, "work");

    let at_gate_again = advanced(
        engine
            .submit_milestone(SESSION, looped.position, evidence(&[]), None)
            .expect("advance off work"),
    );
    let second_challenge = at_gate_again
        .injected
        .approval_challenge
        .expect("a re-entry must issue a NEW challenge");
    assert_ne!(second_challenge, first_challenge);

    // The human's earlier signature is now worthless — this is what makes the
    // approval per-entry rather than a once-and-forever permission.
    let outcome = submit_sig(&mut engine, &stale_sig);
    assert_eq!(reject_reason(outcome), "approval_invalid");

    // Signing the NEW challenge works.
    let fresh_sig = sign(SEED, SESSION, "approve", "human_gate", &second_challenge);
    advanced(submit_sig(&mut engine, &fresh_sig));
}

#[test]
fn garbage_submissions_trip_the_approval_rejection_breaker_not_unbounded_append() {
    // Security-audit FIX 3 (issue #70): before the bound, an agent could spam
    // garbage `approval_signature`s forever, each one a fresh `ApprovalRejected`
    // append, growing the ledger without limit while the same challenge stayed
    // live. With a cap of N, the Nth consecutive rejection trips a typed
    // circuit breaker and fails the session instead.
    let tmp = tempfile::tempdir().unwrap();
    let cap = 3;
    let mut engine = engine_on_with_cap(tmp.path(), cap);
    drive_to_gate(&mut engine);

    // The first cap-1 garbage submissions are ordinary rejections (the session
    // stays on the gate, the challenge stays live).
    for _ in 0..(cap - 1) {
        let outcome = submit_sig(&mut engine, "not_a_valid_signature");
        assert_eq!(reject_reason(outcome), "approval_invalid");
    }

    // The cap-th trips the breaker with a TYPED failure — an Err, not an
    // outcome — and fails the session.
    let err = engine
        .submit_milestone(
            SESSION,
            gate_position(),
            evidence(&[("approval_signature", "not_a_valid_signature")]),
            None,
        )
        .expect_err("the cap-th consecutive rejection must trip the breaker");
    match err {
        FsmError::CircuitBreaker { breaker, .. } => {
            assert_eq!(breaker, CircuitBreakerType::ApprovalRejectionLimit);
        }
        other => panic!("expected an ApprovalRejectionLimit circuit breaker, got {other:?}"),
    }
    assert_eq!(
        engine.get_state(SESSION).unwrap().status,
        SessionStatus::Failed
    );

    // The ledger's ApprovalRejected count is BOUNDED at the cap, not unbounded.
    let events = protocol_ledger::Ledger::replay(SESSION, tmp.path()).expect("replay");
    let rejected = events
        .iter()
        .filter(|e| {
            matches!(
                e.event_type,
                protocol_types::FsmEventType::ApprovalRejected { .. }
            )
        })
        .count();
    assert_eq!(
        rejected as u32, cap,
        "exactly `cap` ApprovalRejected events, then the breaker — not unbounded append"
    );
}

#[test]
fn a_valid_signature_advances_despite_prior_rejections() {
    // The bound only touches the reject path: a VALID signature still opens the
    // gate even after some earlier garbage submissions (which reset on success).
    let tmp = tempfile::tempdir().unwrap();
    let cap = 3;
    let mut engine = engine_on_with_cap(tmp.path(), cap);
    let challenge = drive_to_gate(&mut engine)
        .injected
        .approval_challenge
        .expect("challenge");

    // Two garbage submissions (below the cap of 3) — plain rejections.
    for _ in 0..(cap - 1) {
        assert_eq!(
            reject_reason(submit_sig(&mut engine, "garbage")),
            "approval_invalid"
        );
    }

    // A correctly-bound signature over the live challenge still advances.
    let sig = sign(SEED, SESSION, "approve", "human_gate", &challenge);
    let next = advanced(submit_sig(&mut engine, &sig));
    assert_eq!(next.position.sub_state_id, "chk");
    assert_eq!(
        engine.get_state(SESSION).unwrap().status,
        SessionStatus::Active
    );
}

#[test]
fn a_tampered_valid_signature_is_rejected() {
    // Distinct from the wrong-key, wrong-binding, and garbage-string cases: this
    // starts from a signature the approver's OWN key produced over the LIVE
    // challenge — genuinely valid — and flips a single hex nibble of it. A
    // one-bit change must fail closed (Ed25519 `verify_strict`), and a refusal
    // must not move the session or fail it (it is not a garbage-spam breaker
    // event, just an invalid submission).
    let tmp = tempfile::tempdir().unwrap();
    let mut engine = engine_on(tmp.path());
    let challenge = drive_to_gate(&mut engine)
        .injected
        .approval_challenge
        .expect("challenge");

    let valid = sign(SEED, SESSION, "approve", "human_gate", &challenge);
    let mut chars: Vec<char> = valid.chars().collect();
    // Flip the first nibble to a different, still-valid hex digit.
    chars[0] = if chars[0] == '0' { '1' } else { '0' };
    let tampered: String = chars.into_iter().collect();
    assert_eq!(tampered.len(), valid.len(), "still 64 bytes of hex");
    assert_ne!(tampered, valid, "one nibble actually changed");

    let outcome = submit_sig(&mut engine, &tampered);
    assert_eq!(reject_reason(outcome), "approval_invalid");
    assert_eq!(
        engine.get_state(SESSION).unwrap().status,
        SessionStatus::Active,
        "a single invalid submission must not fail the session"
    );

    // And the honest signature still opens the gate — the refusal was specific
    // to the tampered bytes, not a poisoned gate.
    let next = advanced(submit_sig(&mut engine, &valid));
    assert_eq!(next.position.sub_state_id, "chk");
}

#[test]
fn recovery_mid_approval_replays_the_same_challenge() {
    let tmp = tempfile::tempdir().unwrap();
    let live_challenge = {
        let mut live = engine_on(tmp.path());
        drive_to_gate(&mut live)
            .injected
            .approval_challenge
            .expect("challenge")
    };

    // A brand-new engine over the SAME ledger — a process restart with an
    // empty in-memory map, exactly as the gateway's cache-miss path does.
    let mut recovered = engine_on(tmp.path());
    let state = recovered.recover_session(SESSION).expect("recover");
    assert_eq!(state.position, Some(gate_position()));
    let pending = state
        .pending_approval
        .clone()
        .expect("the parked challenge must survive the restart");
    assert_eq!(
        pending.challenge, live_challenge,
        "the challenge is REPLAYED from the ledger, never regenerated"
    );
    assert_eq!(pending.macro_id, "approve");
    assert_eq!(pending.sub_state_id, "human_gate");

    // A signature the human produced BEFORE the crash still opens the gate.
    let sig = sign(SEED, SESSION, "approve", "human_gate", &live_challenge);
    let next = advanced(submit_sig(&mut recovered, &sig));
    assert_eq!(next.position.sub_state_id, "chk");
}
