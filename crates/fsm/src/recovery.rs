// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! FSM recovery module - replays the ledger to reconstruct SessionState exactly
//! as the live `ProfileFsmEngine` would hold it in memory.

use crate::state::{SessionState, SessionStatus};
use protocol_types::{FsmError, FsmEventType};

/// Replay all events for `session_id` and fold them into a `SessionState`
/// for the nested `ProfileFsmEngine` model, reconstructing
/// `position`/`status`/`macro_iteration` exactly as the live engine would
/// hold them. Macro transitions (`MacroAdvanced`) only carry
/// `from_macro`/`to_macro` — the new macro's first-enabled sub must be
/// looked up in `profile`.
pub fn recover_profile_session(
    session_id: &str,
    ledger: &dyn protocol_ledger::LedgerPort,
    profile: &protocol_types::Profile,
) -> Result<SessionState, FsmError> {
    let events = ledger.replay(session_id)?;
    let mut state: Option<SessionState> = None;

    for event in &events {
        match &event.event_type {
            FsmEventType::SessionStarted { pipeline_id, .. } => {
                let first_macro = profile
                    .pipeline
                    .first()
                    .ok_or_else(|| FsmError::ManifestInvalid("pipeline is empty".to_string()))?;
                let first_sub_idx = crate::injection::first_enabled_index(first_macro)?;
                let first_sub_id = first_macro.sub_states[first_sub_idx].id.clone();
                let initial_context = event
                    .payload
                    .get("initial_context")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

                state = Some(SessionState {
                    session_id: session_id.to_string(),
                    pipeline_id: pipeline_id.clone(),
                    status: SessionStatus::Active,
                    started_at: event.timestamp,
                    updated_at: event.timestamp,
                    consecutive_identical_rejections: 0,
                    last_rejected_evidence_keys: Vec::new(),
                    consecutive_approval_rejections: 0,
                    artifacts: std::collections::HashMap::new(),
                    position: Some(protocol_types::Position {
                        macro_id: first_macro.state_id.clone(),
                        sub_state_id: first_sub_id,
                    }),
                    macro_iteration: 0,
                    initial_context,
                    pending_approval: None,
                });
            }
            FsmEventType::MilestoneRejected { iteration, .. } => {
                if let Some(s) = state.as_mut() {
                    let mut current_keys: Vec<String> = event
                        .payload
                        .get("evidence")
                        .and_then(|v| v.as_object())
                        .map(|m| m.keys().cloned().collect())
                        .unwrap_or_default();
                    current_keys.sort();
                    // Mirrors `ProfileFsmEngine::reject_checklist` (see the
                    // comment there): repeated identical submissions --
                    // including repeated empty ones -- count toward the
                    // oscillation streak, distinguished from "no prior
                    // rejection yet" via `consecutive_identical_rejections > 0`.
                    let has_prior_rejection = s.consecutive_identical_rejections > 0;
                    let is_identical =
                        has_prior_rejection && current_keys == s.last_rejected_evidence_keys;
                    s.last_rejected_evidence_keys = current_keys.clone();
                    if is_identical {
                        s.consecutive_identical_rejections += 1;
                    } else {
                        s.consecutive_identical_rejections = 1;
                    }
                    s.macro_iteration = *iteration;
                }
            }
            FsmEventType::MilestoneSubmitted { .. } => {
                // Flat-engine event; not emitted by ProfileFsmEngine.
            }
            FsmEventType::MilestoneAccepted { .. } => {
                // No position mutation here: a checklist pass is always
                // immediately followed by either `SubStateAdvanced` (n/a for
                // checklists), `MacroAdvanced`, or `SessionCompleted`, each
                // of which carries the actual transition.
            }
            FsmEventType::SubStateAdvanced { to_sub, .. } => {
                if let Some(s) = state.as_mut() {
                    if let Some(pos) = s.position.as_mut() {
                        pos.sub_state_id = to_sub.clone();
                    }
                    // Mirrors the live engine's `reposition`: leaving a
                    // sub-state retires any parked approval challenge. A
                    // challenge for the sub-state we are ARRIVING at is
                    // restored by the `ApprovalChallengeIssued` event the live
                    // engine appends right after this one.
                    s.pending_approval = None;
                }
            }
            FsmEventType::MacroAdvanced { to_macro, .. } => {
                if let Some(s) = state.as_mut() {
                    let next_macro = profile
                        .pipeline
                        .iter()
                        .find(|m| &m.state_id == to_macro)
                        .ok_or_else(|| {
                            FsmError::ManifestInvalid(format!("unknown macro '{to_macro}'"))
                        })?;
                    let next_sub_idx = crate::injection::first_enabled_index(next_macro)?;
                    s.position = Some(protocol_types::Position {
                        macro_id: next_macro.state_id.clone(),
                        sub_state_id: next_macro.sub_states[next_sub_idx].id.clone(),
                    });
                    s.macro_iteration = 0;
                    s.consecutive_identical_rejections = 0;
                    s.last_rejected_evidence_keys.clear();
                    s.pending_approval = None;
                }
            }
            FsmEventType::StepAdvanced { .. } => {
                // Flat-engine event; not emitted by ProfileFsmEngine.
            }
            FsmEventType::CircuitBreakerTriggered { .. } => {
                if let Some(s) = state.as_mut() {
                    s.status = SessionStatus::Failed;
                }
            }
            FsmEventType::SessionCompleted { .. } => {
                if let Some(s) = state.as_mut() {
                    s.status = SessionStatus::Completed;
                    s.position = None;
                }
            }
            FsmEventType::SessionFailed { .. } => {
                if let Some(s) = state.as_mut() {
                    s.status = SessionStatus::Failed;
                }
            }
            FsmEventType::MacroLoopedBack { to_sub, .. } => {
                // SPEC_loopback.md: reposition to the loop target (macro
                // unchanged) exactly as the live engine does in
                // `apply_loop_back`. Do NOT reset macro_iteration /
                // consecutive_identical_rejections / last_rejected_evidence_
                // keys here -- the preceding `MilestoneRejected` in this same
                // ledger entry already set macro_iteration via the arm
                // above, and the bounding invariant requires it survive the
                // loop-back, mirroring the live engine exactly (including
                // mid-loop).
                if let Some(s) = state.as_mut() {
                    if let Some(pos) = s.position.as_mut() {
                        pos.sub_state_id = to_sub.clone();
                    }
                    s.pending_approval = None;
                }
            }
            // SPEC_human_approval.md: the challenge is REPLAYED, never
            // regenerated. Recomputing it would mint a nonce the ledger never
            // witnessed and silently invalidate a signature a human may already
            // have produced against the original -- turning a crash into a
            // "please bother the approver again".
            FsmEventType::ApprovalChallengeIssued {
                macro_id,
                sub_state_id,
                challenge,
            } => {
                if let Some(s) = state.as_mut() {
                    s.pending_approval = Some(protocol_types::ApprovalChallenge {
                        macro_id: macro_id.clone(),
                        sub_state_id: sub_state_id.clone(),
                        challenge: challenge.clone(),
                    });
                    // Mirrors the live engine's `enter_sub`: a fresh challenge
                    // resets the approval-rejection streak (FIX 3).
                    s.consecutive_approval_rejections = 0;
                }
            }
            FsmEventType::ApprovalAccepted { .. } => {
                // The gate was passed; the challenge is spent. The
                // `SubStateAdvanced` that follows carries the actual movement.
                if let Some(s) = state.as_mut() {
                    s.pending_approval = None;
                }
            }
            FsmEventType::ApprovalRejected { .. } => {
                // Audit-only for position: a refused submission moves nothing
                // and spends nothing, so the parked challenge survives it
                // (exactly as in the live engine). It DOES advance the
                // rejection streak the `ApprovalRejectionLimit` breaker bounds
                // (FIX 3), so mirror the live engine's increment here; a
                // `CircuitBreakerTriggered` (handled above) marks the trip.
                if let Some(s) = state.as_mut() {
                    s.consecutive_approval_rejections += 1;
                }
            }
            FsmEventType::ArtifactStored { artifact_ref } => {
                // Rehydrate the tracked-artifact set exactly as the live engine
                // holds it (keyed by the ref's unique on-disk path), so the
                // completion-time integrity check runs against a recovered
                // session too.
                if let Some(s) = state.as_mut() {
                    s.artifacts.insert(
                        artifact_ref.path.to_string_lossy().into_owned(),
                        artifact_ref.clone(),
                    );
                }
            }
        }

        if let Some(s) = state.as_mut() {
            s.updated_at = event.timestamp;
        }
    }

    state.ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))
}

/// Gateway-facing recovery entry point (SPEC_crash_recovery.md §2): rebuild
/// `session_id`'s `SessionState` by folding `self.ledger`'s events against
/// `self.profile` (already loaded + normalized by `ProfileFsmEngine::new`),
/// insert it into `self.sessions`, and return a reference. Used by the
/// gateway on a cache miss -- the caller is expected to have built a fresh
/// engine (profile reloaded + revalidated, ledger attached to the existing
/// on-disk file) before calling this. Additive only: no existing caller of
/// `ProfileFsmEngine` changes.
impl<L: protocol_ledger::LedgerPort> crate::profile_engine::ProfileFsmEngine<L> {
    pub fn recover_session(
        &mut self,
        session_id: &str,
    ) -> Result<&SessionState, protocol_types::FsmError> {
        let state = recover_profile_session(
            session_id,
            &self.ledger as &dyn protocol_ledger::LedgerPort,
            &self.profile,
        )?;
        self.sessions.insert(session_id.to_string(), state);
        Ok(self.sessions.get(session_id).expect("just inserted"))
    }
}

#[cfg(test)]
mod recover_session_tests {
    use crate::config::FsmConfig;
    use crate::profile_engine::{ProfileFsmEngine, StepOutcome};
    use protocol_library::Library;
    use protocol_types::{
        ChecklistEvidence, Injection, OutputContract, Profile, ProfileSettings, StateDef,
        SubStateDef, SubStateType,
    };
    use std::collections::HashMap;

    fn evidence(pairs: &[(&str, serde_json::Value)]) -> ChecklistEvidence {
        let mut m: ChecklistEvidence = HashMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), v.clone());
        }
        m
    }

    /// Two macros (`draft` non-final, `ship` final), mirroring the shape of
    /// the crate's other acceptance fixtures, kept minimal since this test
    /// only cares about `recover_session` matching the live engine mid-way
    /// through `draft`.
    fn profile() -> Profile {
        Profile {
            name: "recovery-test".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            protected: false,
            created_at: String::new(),
            cloned_from: None,
            settings: ProfileSettings {
                oscillation_detection_threshold: 3,
                auto_loop_on_checklist_failure: false,
                global_timeout_seconds: 3600,
                ..ProfileSettings::default()
            },
            output_contract: None::<OutputContract>,
            pipeline: vec![
                StateDef {
                    state_id: "draft".to_string(),
                    name: "Draft".to_string(),
                    description: String::new(),
                    system_prompt: None,
                    enabled: true,
                    max_iterations: 5,
                    loop_state: false,
                    icon: None,
                    sub_states: vec![
                        SubStateDef {
                            id: "s1".to_string(),
                            sub_state_type: SubStateType::Inject,
                            name: "Step 1".to_string(),
                            description: String::new(),
                            enabled: true,
                            criteria: None,
                            inject: Some(Injection {
                                prompt: Some("do step 1".to_string()),
                                skill: None,
                                protocol: None,
                                context: None,
                            }),
                            verify: None,
                            approver_pubkey: None,
                            approval_prompt: None,
                            hooks: Vec::new(),
                        },
                        SubStateDef {
                            id: "chk1".to_string(),
                            sub_state_type: SubStateType::Checklist,
                            name: "Draft checklist".to_string(),
                            description: String::new(),
                            enabled: true,
                            criteria: Some(vec!["done".to_string()]),
                            inject: None,
                            verify: None,
                            approver_pubkey: None,
                            approval_prompt: None,
                            hooks: Vec::new(),
                        },
                    ],
                    hooks: Vec::new(),
                },
                StateDef {
                    state_id: "ship".to_string(),
                    name: "Ship".to_string(),
                    description: String::new(),
                    system_prompt: None,
                    enabled: true,
                    max_iterations: 5,
                    loop_state: false,
                    icon: None,
                    sub_states: vec![SubStateDef {
                        id: "chk2".to_string(),
                        sub_state_type: SubStateType::Checklist,
                        name: "Ship checklist".to_string(),
                        description: String::new(),
                        enabled: true,
                        criteria: Some(vec!["shipped".to_string()]),
                        inject: None,
                        verify: None,
                        approver_pubkey: None,
                        approval_prompt: None,
                        hooks: Vec::new(),
                    }],
                    hooks: Vec::new(),
                },
            ],
        }
    }

    /// Builds a `ProfileFsmEngine<protocol_ledger::Ledger>` attached to a
    /// real on-disk ledger file under `dir` -- the same file-backed
    /// `LedgerPort` impl the gateway uses in production, needed so a
    /// SECOND, independently-constructed engine can attach to the same
    /// file and recover from it, faithfully exercising the "different
    /// process/worker" scenario `recover_session` targets.
    fn engine_on_dir(dir: &std::path::Path) -> ProfileFsmEngine<protocol_ledger::Ledger> {
        let library = Library::new(dir.join("library"));
        let ledger = protocol_ledger::Ledger::new("sess-1", dir).unwrap();
        ProfileFsmEngine::new(profile(), library, ledger, FsmConfig::default())
    }

    #[test]
    fn recover_session_matches_live_engine_mid_session() {
        let tmp = tempfile::tempdir().unwrap();

        // Live engine: start, advance past s1 into chk1, submit one
        // (rejected) milestone to also exercise macro_iteration tracking,
        // then advance for real into ship's first sub-state.
        let mut live = engine_on_dir(tmp.path());
        let start = live.start_session("sess-1", None).unwrap();
        assert_eq!(start.position.sub_state_id, "s1");

        let at_chk1 = match live
            .submit_milestone("sess-1", start.position.clone(), evidence(&[]), None)
            .unwrap()
        {
            StepOutcome::Advanced(view) => view,
            other => panic!("expected Advanced, got {other:?}"),
        };
        assert_eq!(at_chk1.position.sub_state_id, "chk1");

        // A rejection first, to bump macro_iteration on the live engine.
        match live
            .submit_milestone("sess-1", at_chk1.position.clone(), evidence(&[]), None)
            .unwrap()
        {
            StepOutcome::Rejected { .. } => {}
            other => panic!("expected Rejected, got {other:?}"),
        }

        // Then a real pass, advancing into the final `ship` macro.
        let advanced_ship = match live
            .submit_milestone(
                "sess-1",
                at_chk1.position.clone(),
                evidence(&[("done", serde_json::json!(true))]),
                None,
            )
            .unwrap()
        {
            StepOutcome::Advanced(view) => view,
            other => panic!("expected Advanced into ship, got {other:?}"),
        };
        assert_eq!(advanced_ship.position.macro_id, "ship");

        let live_state = live.get_state("sess-1").unwrap().clone();

        // A SECOND, fresh engine attached to the SAME ledger dir/session --
        // simulating a process restart with an empty in-memory map.
        let mut recovered_engine = engine_on_dir(tmp.path());
        let recovered_state = recovered_engine.recover_session("sess-1").unwrap();

        assert_eq!(recovered_state.position, live_state.position);
        assert_eq!(recovered_state.status, live_state.status);
        assert_eq!(recovered_state.macro_iteration, live_state.macro_iteration);
        assert_eq!(
            recovered_state.consecutive_identical_rejections,
            live_state.consecutive_identical_rejections
        );

        // The recovered engine's `self.sessions` entry is the same object
        // `recover_session` returned a reference into.
        assert_eq!(
            recovered_engine.get_state("sess-1").unwrap().position,
            live_state.position
        );
    }
}
