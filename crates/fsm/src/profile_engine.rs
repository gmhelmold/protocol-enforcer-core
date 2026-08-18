// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! `ProfileFsmEngine` — the nested (Tier 1 macro / Tier 2 sub-state) engine
//! (SPEC_v3.md §3, §9, WP3). Drives a single `Profile` for any number of
//! concurrent sessions, rendering `RenderedInjection`s via `protocol-library`
//! and enforcing presence-only checklists, circuit breakers, and the
//! optional output contract.
//!
//! WP4 severed the flat `FsmEngine`'s agent coupling (`execute_step_with_agent`
//! removed, `protocol-agent` dropped from this crate's deps). WP6 retired
//! the flat `FsmEngine`/`ManifestIR` served path itself — this is now the
//! sole engine over the served path.

use crate::config::FsmConfig;
pub use crate::injection::RenderedInjection;
use crate::injection::{
    first_enabled_index, missing_criteria, next_enabled_index, render_injection,
};
use chrono::Utc;
use protocol_artifacts::ArtifactStore;
use protocol_ledger::LedgerPort;
use protocol_library::Library;
use protocol_types::{
    ChecklistEvidence, CircuitBreakerType, FsmError, FsmEvent, FsmEventType, Position, Profile,
    SessionState, SessionStatus, SubStateType,
};
use std::collections::HashMap;
use uuid::Uuid;

/// A single stepping result: where the session now is, what to inject, and
/// its status (SPEC_v3 §9).
#[derive(Debug, Clone, PartialEq)]
pub struct StepView {
    pub position: Position,
    pub injected: RenderedInjection,
    pub macro_name: String,
    pub session_state: SessionStatus,
}

/// Result of `submit_milestone` (SPEC_v3 §9). Circuit breakers do NOT appear
/// here — they surface as `Err(FsmError::CircuitBreaker{..})`.
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    Advanced(StepView),
    Rejected {
        rejected_items: Vec<String>,
        reason: String,
        position: Position,
    },
    /// A checklist rejection loop-backed to the macro's first enabled
    /// Execute sub-state (SPEC_loopback.md) instead of staying on the
    /// checklist. Carries BOTH why it rejected (`rejected_items`) AND the
    /// new position/injection (`view`) the caller must now render.
    LoopedBack {
        rejected_items: Vec<String>,
        view: StepView,
    },
    Completed {
        contract_path: Option<String>,
    },
}

/// Drives the nested forced state machine for one `Profile` across any
/// number of concurrent sessions (SPEC_v3 §3, §9).
pub struct ProfileFsmEngine<L: LedgerPort> {
    pub(crate) profile: Profile,
    pub(crate) library: Library,
    pub(crate) ledger: L,
    pub(crate) config: FsmConfig,
    pub(crate) sessions: HashMap<String, SessionState>,
}

impl<L: LedgerPort> ProfileFsmEngine<L> {
    /// SPEC_config_layer.md "Macro-level `enabled` — normalize at load":
    /// `profile` is normalized to its enabled-macros-only pipeline ONCE
    /// here, so every stepping/navigation index below
    /// (`pipeline.first()`/`.last()`/`[idx+1]`/`len()-1`) is already
    /// correct against disabled macros with zero further logic changes.
    pub fn new(profile: Profile, library: Library, ledger: L, config: FsmConfig) -> Self {
        Self {
            profile: profile.with_enabled_macros_only(),
            library,
            ledger,
            config,
            sessions: HashMap::new(),
        }
    }

    /// SPEC_v3 §3 `start_session`: validate the profile, position at the
    /// first macro's first enabled sub, store `initial_context` (merged
    /// into the first sub's rendered context), append `SessionStarted`.
    pub fn start_session(
        &mut self,
        session_id: &str,
        initial_context: Option<serde_json::Value>,
    ) -> Result<StepView, FsmError> {
        protocol_manifest::validate_profile(&self.profile, &self.library)
            .map_err(|violations| FsmError::ManifestInvalid(violations.join("; ")))?;

        let first_macro = self
            .profile
            .pipeline
            .first()
            .ok_or_else(|| FsmError::ManifestInvalid("pipeline is empty".to_string()))?;
        let first_sub_idx = first_enabled_index(first_macro)?;
        let first_sub = &first_macro.sub_states[first_sub_idx];

        let now = Utc::now();
        let initial_context_value = initial_context.clone().unwrap_or(serde_json::Value::Null);
        let position = Position {
            macro_id: first_macro.state_id.clone(),
            sub_state_id: first_sub.id.clone(),
        };

        let state = SessionState {
            session_id: session_id.to_string(),
            pipeline_id: self.profile.name.clone(),
            status: SessionStatus::Active,
            started_at: now,
            updated_at: now,
            consecutive_identical_rejections: 0,
            last_rejected_evidence_keys: Vec::new(),
            consecutive_approval_rejections: 0,
            artifacts: HashMap::new(),
            position: Some(position.clone()),
            macro_iteration: 0,
            initial_context: initial_context_value.clone(),
            pending_approval: None,
        };

        let macro_name = first_macro.name.clone();
        let pipeline_id = self.profile.name.clone();
        let manifest_version = self.profile.version.clone();

        // The session must be in the map BEFORE `enter_sub`, which parks a
        // challenge on it when the first sub-state is a `human_approval` gate.
        // If either step below fails the session is removed again, so a failed
        // start leaves no half-born session behind (the pre-approval behaviour).
        self.sessions.insert(session_id.to_string(), state);
        let entered = self
            .append(
                session_id,
                FsmEventType::SessionStarted {
                    pipeline_id,
                    manifest_version,
                    // Computed by the gateway (it has the file bytes) and carried
                    // in via FsmConfig; the engine only copies it (§5).
                    profile_sha256: self.config.profile_sha256.clone(),
                },
                &position,
                serde_json::json!({ "initial_context": initial_context_value }),
            )
            .and_then(|()| self.enter_sub(session_id, 0, first_sub_idx, initial_context.as_ref()));

        let injected = match entered {
            Ok((_, injected)) => injected,
            Err(e) => {
                self.sessions.remove(session_id);
                return Err(e);
            }
        };

        Ok(StepView {
            position,
            injected,
            macro_name,
            session_state: SessionStatus::Active,
        })
    }

    /// SPEC_v3 §3 `submit_milestone` steps 1-4.
    pub fn submit_milestone(
        &mut self,
        session_id: &str,
        pos: Position,
        evidence: ChecklistEvidence,
        output: Option<serde_json::Value>,
    ) -> Result<StepOutcome, FsmError> {
        // Step 1: status Active else SESSION_NOT_FOUND; pos matches tracked
        // position else STEP_MISMATCH.
        let started_at = {
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?;
            if session.status != SessionStatus::Active {
                return Err(FsmError::SessionInactive {
                    session_id: session_id.to_string(),
                    status: session.status.clone(),
                });
            }
            let current = session
                .position
                .as_ref()
                .ok_or_else(|| FsmError::Internal("active session has no position".to_string()))?;
            if *current != pos {
                return Err(FsmError::StepMismatch {
                    expected: format!("{}/{}", current.macro_id, current.sub_state_id),
                    actual: format!("{}/{}", pos.macro_id, pos.sub_state_id),
                });
            }
            session.started_at
        };

        // Step 2: global timeout.
        let elapsed = (Utc::now() - started_at).num_seconds().max(0) as u64;
        if self.config.global_timeout_seconds != 0 && elapsed >= self.config.global_timeout_seconds
        {
            let breaker = CircuitBreakerType::GlobalTimeout;
            self.append(
                session_id,
                FsmEventType::CircuitBreakerTriggered {
                    breaker: breaker.clone(),
                    details: serde_json::json!({
                        "elapsed_seconds": elapsed,
                        "limit_seconds": self.config.global_timeout_seconds
                    }),
                },
                &pos,
                serde_json::json!({}),
            )?;
            if let Some(session) = self.sessions.get_mut(session_id) {
                session.status = SessionStatus::Failed;
            }
            return Err(FsmError::CircuitBreaker {
                breaker,
                details: format!(
                    "Global timeout ({} s) exceeded after {} s",
                    self.config.global_timeout_seconds, elapsed
                ),
            });
        }

        let macro_idx = self
            .profile
            .pipeline
            .iter()
            .position(|m| m.state_id == pos.macro_id)
            .ok_or_else(|| FsmError::Internal(format!("unknown macro '{}'", pos.macro_id)))?;
        let sub_idx = self.profile.pipeline[macro_idx]
            .sub_states
            .iter()
            .position(|s| s.id == pos.sub_state_id)
            .ok_or_else(|| {
                FsmError::Internal(format!("unknown sub-state '{}'", pos.sub_state_id))
            })?;
        let sub_type = self.profile.pipeline[macro_idx].sub_states[sub_idx]
            .sub_state_type
            .clone();

        match sub_type {
            SubStateType::Checklist => {
                self.step_checklist(session_id, macro_idx, &pos, evidence, output)
            }
            // The one sub-state whose submission is VERIFIED, not merely
            // observed (SPEC_human_approval.md) — see `approval.rs`.
            SubStateType::HumanApproval => {
                self.step_approval(session_id, macro_idx, sub_idx, &pos, evidence)
            }
            _ => self.advance_plain(session_id, macro_idx, sub_idx, &pos),
        }
    }

    pub fn get_state(&self, session_id: &str) -> Option<&SessionState> {
        self.sessions.get(session_id)
    }

    /// SPEC_v3 §9 / FR-25: offload an artifact THROUGH the enforcer's own
    /// `ArtifactStore`. The store hashes the received bytes, so the returned
    /// `sha256` is ENFORCER-authored (never an agent claim) — which is what
    /// keeps the completion-time integrity check passive ("is my stored blob
    /// unchanged?") rather than a judgement of an agent's assertion. Appends an
    /// `ArtifactStored` event (so recovery rehydrates the set) and tracks the
    /// ref on the session, keyed by its unique on-disk path.
    pub fn store_artifact(
        &mut self,
        session_id: &str,
        step_id: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<protocol_types::ArtifactRef, FsmError> {
        // Only an active session may accumulate artifacts.
        let pos = {
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?;
            if session.status != SessionStatus::Active {
                return Err(FsmError::SessionInactive {
                    session_id: session_id.to_string(),
                    status: session.status.clone(),
                });
            }
            session.position.clone().unwrap_or_else(|| Position {
                macro_id: String::new(),
                sub_state_id: String::new(),
            })
        };

        let mut store = ArtifactStore::new(session_id, &self.config.artifact_root)?;
        let artifact_ref = store.store_with_mime(step_id, data, mime_type)?;

        self.append(
            session_id,
            FsmEventType::ArtifactStored {
                artifact_ref: artifact_ref.clone(),
            },
            &pos,
            serde_json::json!({}),
        )?;

        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?;
        session.artifacts.insert(
            artifact_ref.path.to_string_lossy().into_owned(),
            artifact_ref.clone(),
        );
        session.updated_at = Utc::now();

        Ok(artifact_ref)
    }

    /// SPEC_v3 §9: every artifact offloaded through the enforcer must still hash
    /// to its enforcer-authored `sha256` before the session may complete. This
    /// is an integrity check on our OWN stored blobs (passive) — not a judgement
    /// of any agent claim. Called by `complete_final_macro`; on failure the
    /// caller leaves the session on the final state (like an output-contract
    /// violation). No-op when the session tracked no artifacts.
    pub(crate) fn verify_session_artifacts(&self, session_id: &str) -> Result<(), FsmError> {
        let refs: Vec<protocol_types::ArtifactRef> = match self.sessions.get(session_id) {
            Some(s) if !s.artifacts.is_empty() => s.artifacts.values().cloned().collect(),
            _ => return Ok(()),
        };
        let store = ArtifactStore::new(session_id, &self.config.artifact_root)?;
        for aref in &refs {
            if !store.verify(aref)? {
                return Err(FsmError::OutputContractViolation(format!(
                    "artifact integrity check failed (content changed since offload): {}",
                    aref.path.display()
                )));
            }
        }
        Ok(())
    }

    // -- internals ---------------------------------------------------

    /// SPEC_v3 §3 step 3: non-Checklist sub, plain ack.
    pub(crate) fn advance_plain(
        &mut self,
        session_id: &str,
        macro_idx: usize,
        sub_idx: usize,
        pos: &Position,
    ) -> Result<StepOutcome, FsmError> {
        let macro_def = &self.profile.pipeline[macro_idx];
        let next_idx = next_enabled_index(macro_def, sub_idx);
        let next_position = Position {
            macro_id: macro_def.state_id.clone(),
            sub_state_id: macro_def.sub_states[next_idx].id.clone(),
        };
        let macro_name = macro_def.name.clone();
        let from_sub = pos.sub_state_id.clone();
        let to_sub = next_position.sub_state_id.clone();

        self.append(
            session_id,
            FsmEventType::SubStateAdvanced { from_sub, to_sub },
            &next_position,
            serde_json::json!({}),
        )?;
        self.reposition(session_id, &next_position)?;
        let (_, injected) = self.enter_sub(session_id, macro_idx, next_idx, None)?;

        let status = self
            .sessions
            .get(session_id)
            .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?
            .status
            .clone();

        Ok(StepOutcome::Advanced(StepView {
            position: next_position,
            injected,
            macro_name,
            session_state: status,
        }))
    }

    /// Move the tracked position to `next_position`, retiring any challenge the
    /// session was holding. Leaving a `human_approval` gate ALWAYS drops its
    /// challenge — re-entering mints a new one — which is what makes an old
    /// signature worthless after a loop-back or a second macro iteration.
    pub(crate) fn reposition(
        &mut self,
        session_id: &str,
        next_position: &Position,
    ) -> Result<(), FsmError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?;
        session.position = Some(next_position.clone());
        session.pending_approval = None;
        session.updated_at = Utc::now();
        Ok(())
    }

    /// Land on `pipeline[macro_idx].sub_states[sub_idx]`: render its injection
    /// and, when it is a `human_approval` gate, mint a fresh challenge, LEDGER
    /// it (`ApprovalChallengeIssued`), park it on the session, and hand it to
    /// the caller inside the step payload. Ledgering before parking is what
    /// lets recovery replay the very same nonce instead of inventing one
    /// (SPEC_human_approval.md).
    ///
    /// Every path that lands a session on a sub-state goes through here
    /// (`start_session`, `advance_plain`, `advance_macro`), so a gate cannot be
    /// entered by some route that forgets to challenge it.
    pub(crate) fn enter_sub(
        &mut self,
        session_id: &str,
        macro_idx: usize,
        sub_idx: usize,
        initial_context: Option<&serde_json::Value>,
    ) -> Result<(Position, RenderedInjection), FsmError> {
        let macro_def = &self.profile.pipeline[macro_idx];
        let sub = &macro_def.sub_states[sub_idx];
        let position = Position {
            macro_id: macro_def.state_id.clone(),
            sub_state_id: sub.id.clone(),
        };
        let mut injected = render_injection(sub, macro_def, &self.library, initial_context)?;
        let issued = crate::approval::issue_for(sub, &position)?;

        if let Some(challenge) = issued {
            self.append(
                session_id,
                FsmEventType::ApprovalChallengeIssued {
                    macro_id: challenge.macro_id.clone(),
                    sub_state_id: challenge.sub_state_id.clone(),
                    challenge: challenge.challenge.clone(),
                },
                &position,
                serde_json::json!({}),
            )?;
            injected.approval_challenge = Some(challenge.challenge.clone());
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?;
            session.pending_approval = Some(challenge);
            // Fresh gate entry — the rejection streak is scoped to this
            // challenge (security-audit FIX 3), so it starts at 0.
            session.consecutive_approval_rejections = 0;
        }

        Ok((position, injected))
    }

    /// SPEC_v3 §3 step 4: current sub is the macro's Checklist.
    fn step_checklist(
        &mut self,
        session_id: &str,
        macro_idx: usize,
        pos: &Position,
        evidence: ChecklistEvidence,
        output: Option<serde_json::Value>,
    ) -> Result<StepOutcome, FsmError> {
        let macro_def = &self.profile.pipeline[macro_idx];
        let sub = macro_def
            .sub_states
            .iter()
            .find(|s| s.id == pos.sub_state_id)
            .expect("caller resolved sub_idx from this macro");
        let criteria = sub.criteria.clone().unwrap_or_default();
        let missing = missing_criteria(&criteria, &evidence);

        if !missing.is_empty() {
            return self.reject_checklist(session_id, macro_idx, pos, missing, evidence);
        }

        let is_final_macro = macro_idx == self.profile.pipeline.len() - 1;
        if !is_final_macro {
            self.advance_macro(session_id, macro_idx, pos)
        } else {
            self.complete_final_macro(session_id, pos, output)
        }
    }

    pub(crate) fn append(
        &mut self,
        session_id: &str,
        event_type: FsmEventType,
        pos: &Position,
        payload: serde_json::Value,
    ) -> Result<(), FsmError> {
        let event = FsmEvent {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            session_id: session_id.to_string(),
            event_type,
            step_id: Some(format!("{}/{}", pos.macro_id, pos.sub_state_id)),
            payload,
        };
        self.ledger.append(&event)?;
        Ok(())
    }
}
