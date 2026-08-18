// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Rejection, loop-back gating, macro advancement, and final-macro completion
//! for `ProfileFsmEngine` (SPEC_v3.md §9). Split out of `profile_engine.rs`
//! (which keeps `start_session`/`submit_milestone`/`get_state` and the stepping
//! primitives) to stay under the file-size cap. These are inherent methods on
//! the same engine type; `submit_milestone` drives them.

use chrono::Utc;
use protocol_ledger::LedgerPort;
use protocol_types::{
    ChecklistEvidence, CircuitBreakerType, FsmError, FsmEventType, OutputContract, Position,
    SessionStatus,
};
use sha2::{Digest, Sha256};

use crate::injection::first_enabled_index;
use crate::profile_engine::{ProfileFsmEngine, StepOutcome, StepView};

impl<L: LedgerPort> ProfileFsmEngine<L> {
    pub(crate) fn reject_checklist(
        &mut self,
        session_id: &str,
        macro_idx: usize,
        pos: &Position,
        missing: Vec<String>,
        evidence: ChecklistEvidence,
    ) -> Result<StepOutcome, FsmError> {
        let macro_def = &self.profile.pipeline[macro_idx];
        let effective_max_iter = macro_def.max_iterations;
        let oscillation_threshold = self.profile.settings.oscillation_detection_threshold;

        let mut current_keys: Vec<String> = evidence.keys().cloned().collect();
        current_keys.sort();

        let (macro_iteration, consecutive_identical) = {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?;
            session.macro_iteration += 1;
            // Repeated submissions with the SAME evidence-key shape count
            // toward the oscillation streak -- including repeated EMPTY
            // submissions (a client that keeps submitting no evidence at
            // all). `last_rejected_evidence_keys` is empty both before the
            // first rejection in this macro and after an empty-keys
            // rejection, so `consecutive_identical_rejections > 0` (reset to
            // 0 whenever the macro advances/starts) is used to distinguish
            // "no prior rejection yet" from "prior rejection was also
            // empty".
            let has_prior_rejection = session.consecutive_identical_rejections > 0;
            let is_identical =
                has_prior_rejection && current_keys == session.last_rejected_evidence_keys;
            session.last_rejected_evidence_keys = current_keys.clone();
            if is_identical {
                session.consecutive_identical_rejections += 1;
            } else {
                session.consecutive_identical_rejections = 1;
            }
            (
                session.macro_iteration,
                session.consecutive_identical_rejections,
            )
        };

        self.append(
            session_id,
            FsmEventType::MilestoneRejected {
                rejected_items: missing.clone(),
                reason: "checklist_incomplete".to_string(),
                iteration: macro_iteration,
            },
            pos,
            serde_json::json!({ "evidence": evidence }),
        )?;

        if macro_iteration >= effective_max_iter {
            let breaker = CircuitBreakerType::MaxIterations;
            self.append(
                session_id,
                FsmEventType::CircuitBreakerTriggered {
                    breaker: breaker.clone(),
                    details: serde_json::json!({ "macro": pos.macro_id, "iterations": macro_iteration }),
                },
                pos,
                serde_json::json!({}),
            )?;
            if let Some(session) = self.sessions.get_mut(session_id) {
                session.status = SessionStatus::Failed;
            }
            return Err(FsmError::CircuitBreaker {
                breaker,
                details: format!("Max iterations ({effective_max_iter}) exceeded"),
            });
        }

        if consecutive_identical >= oscillation_threshold {
            let breaker = CircuitBreakerType::RepetitiveLoop;
            self.append(
                session_id,
                FsmEventType::CircuitBreakerTriggered {
                    breaker: breaker.clone(),
                    details: serde_json::json!({
                        "macro": pos.macro_id,
                        "consecutive_identical": consecutive_identical
                    }),
                },
                pos,
                serde_json::json!({}),
            )?;
            if let Some(session) = self.sessions.get_mut(session_id) {
                session.status = SessionStatus::Failed;
            }
            return Err(FsmError::CircuitBreaker {
                breaker,
                details: format!(
                    "Repetitive loop detected: {consecutive_identical} consecutive identical rejections"
                ),
            });
        }

        // SPEC_loopback.md: loop-back is the AND-gate of a global kill-switch
        // and a per-macro opt-in, checked AFTER both circuit breakers above
        // (a rejection that trips a breaker fails the session even with loop
        // enabled -- macro_iteration/oscillation are never reset). See
        // `loop_back.rs::apply_loop_back` for the append+reposition (split
        // out to keep this file under the LOC cap).
        let loop_enabled = self.profile.settings.auto_loop_on_checklist_failure
            && self.profile.pipeline[macro_idx].loop_state;

        if loop_enabled {
            return self.apply_loop_back(session_id, macro_idx, pos, missing, macro_iteration);
        }

        Ok(StepOutcome::Rejected {
            rejected_items: missing,
            reason: "checklist_incomplete".to_string(),
            position: pos.clone(),
        })
    }

    pub(crate) fn advance_macro(
        &mut self,
        session_id: &str,
        macro_idx: usize,
        pos: &Position,
    ) -> Result<StepOutcome, FsmError> {
        self.append(
            session_id,
            FsmEventType::MilestoneAccepted { next_step_id: None },
            pos,
            serde_json::json!({}),
        )?;

        let next_macro_idx = macro_idx + 1;
        let next_macro = &self.profile.pipeline[next_macro_idx];
        let next_sub_idx = first_enabled_index(next_macro)?;
        let next_position = Position {
            macro_id: next_macro.state_id.clone(),
            sub_state_id: next_macro.sub_states[next_sub_idx].id.clone(),
        };
        let macro_name = next_macro.name.clone();
        let from_macro = pos.macro_id.clone();
        let to_macro = next_macro.state_id.clone();

        self.append(
            session_id,
            FsmEventType::MacroAdvanced {
                from_macro,
                to_macro,
            },
            &next_position,
            serde_json::json!({}),
        )?;

        // `reposition` also retires any parked approval challenge; the
        // macro-scoped counters below are this transition's own concern.
        self.reposition(session_id, &next_position)?;
        {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?;
            session.macro_iteration = 0;
            session.consecutive_identical_rejections = 0;
            session.last_rejected_evidence_keys.clear();
        }

        // Landing on the next macro's first sub-state issues its challenge when
        // that sub-state is a `human_approval` gate (SPEC_human_approval.md).
        let (_, injected) = self.enter_sub(session_id, next_macro_idx, next_sub_idx, None)?;
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

    pub(crate) fn complete_final_macro(
        &mut self,
        session_id: &str,
        pos: &Position,
        output: Option<serde_json::Value>,
    ) -> Result<StepOutcome, FsmError> {
        // SPEC_v3 §9: verify all offloaded artifacts' integrity BEFORE validating
        // the output contract; a tampered blob keeps the session on the final
        // state rather than completing.
        self.verify_session_artifacts(session_id)?;
        let contract = self.profile.output_contract.clone();

        // `(path, output_sha256)` — the digest is of the exact bytes written,
        // computed by `validate_and_write_output` so nothing is re-read or
        // re-serialized to hash it (§8). `None` when the profile has no contract.
        let contract_written = match &contract {
            Some(contract) => Some(self.validate_and_write_output(session_id, contract, output)?),
            None => None,
        };
        let contract_path = contract_written.as_ref().map(|(path, _)| path.clone());
        let output_sha256 = contract_written.as_ref().map(|(_, digest)| digest.clone());

        self.append(
            session_id,
            FsmEventType::MilestoneAccepted { next_step_id: None },
            pos,
            serde_json::json!({}),
        )?;
        self.append(
            session_id,
            FsmEventType::SessionCompleted {
                final_artifact_path: contract_path.clone().unwrap_or_default(),
                output_sha256,
            },
            pos,
            serde_json::json!({}),
        )?;

        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?;
        session.status = SessionStatus::Completed;
        session.position = None;
        session.updated_at = Utc::now();

        Ok(StepOutcome::Completed { contract_path })
    }

    /// Validate `output` against `contract.schema`, verify tracked
    /// artifacts, and write it to the resolved destination on success.
    /// Returns `(written_path, output_sha256)` where the digest is the
    /// lowercase-hex SHA-256 of exactly the bytes written — computed here, not
    /// by re-reading the file, so the double-serialization hazard §1 removes is
    /// not reintroduced (§8). On failure the session is left untouched (stays on
    /// state) and `OutputContractViolation` is returned.
    fn validate_and_write_output(
        &self,
        session_id: &str,
        contract: &OutputContract,
        output: Option<serde_json::Value>,
    ) -> Result<(String, String), FsmError> {
        let output = output.ok_or_else(|| {
            FsmError::OutputContractViolation(
                "output is required on the final checklist".to_string(),
            )
        })?;

        protocol_contract::validate_output(&output, &contract.schema)
            .map_err(|e| FsmError::OutputContractViolation(e.to_string()))?;

        // Every artifact tracked on the session must verify before the
        // session may complete. The nested `submit_milestone` signature
        // (SPEC_v3 §9) has no artifact-registration path yet, so this set
        // is empty today; the check stays here as the enforcement point the
        // moment artifacts are tracked.
        let destination = contract.destination.replace("{session_id}", session_id);
        // Defense in depth: never trust the validator alone. Confine the
        // resolved destination under the process cwd (the contract's base) so a
        // profile whose destination escaped the strict validator — or one
        // reaching a driver that skipped it — still cannot steer this write
        // outside the sandbox. A live exploit wrote a file OUTSIDE the base via
        // an absolute destination; this is the enforcement point that refuses
        // it regardless of the validator.
        let confined = confine_output_destination(&self.config.output_base, &destination)?;
        if let Some(parent) = confined.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| FsmError::Internal(format!("failed to create output dir: {e}")))?;
            }
        }
        // Hash the exact bytes we are about to write, so `output_sha256`
        // commits to the on-disk deliverable without a second serialization.
        let bytes = serde_json::to_vec_pretty(&output).unwrap_or_default();
        let output_sha256 = format!("{:x}", Sha256::digest(&bytes));
        std::fs::write(&confined, &bytes)
            .map_err(|e| FsmError::Internal(format!("failed to write output contract: {e}")))?;

        Ok((confined.to_string_lossy().into_owned(), output_sha256))
    }
}

/// Resolve `destination` against `base` (the output-contract confinement base,
/// `FsmConfig::output_base`, which defaults to the process cwd) and confine it
/// there, refusing any path that escapes. There is no separate profile-
/// configurable output base — `artifact_root` governs the artifact store, not
/// the output contract — so by default the cwd IS the contract's base.
///
/// The path is normalized LEXICALLY (`.`/`..` collapsed without touching the
/// filesystem) so the check does not depend on the target existing, then the
/// result must remain under `base`. Rejects absolute paths and embedded NUL
/// up front; both are unambiguous escapes.
fn confine_output_destination(
    base: &std::path::Path,
    destination: &str,
) -> Result<std::path::PathBuf, FsmError> {
    use std::path::{Component, PathBuf};

    if destination.contains('\0') {
        return Err(FsmError::OutputContractViolation(
            "output_contract.destination must not contain a NUL byte".to_string(),
        ));
    }
    let raw = std::path::Path::new(destination);
    if raw.is_absolute() {
        return Err(FsmError::OutputContractViolation(format!(
            "output_contract.destination must be relative to the output base, got absolute '{destination}'"
        )));
    }

    // Lexically normalize base.join(raw): fold away CurDir, and pop the last
    // real segment on each ParentDir. A `..` that would pop past the base is an
    // escape — refuse it rather than clamp silently.
    let mut normalized = PathBuf::new();
    for comp in base.join(raw).components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(FsmError::OutputContractViolation(format!(
                        "output_contract.destination escapes the output base: '{destination}'"
                    )));
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }

    if !normalized.starts_with(base) {
        return Err(FsmError::OutputContractViolation(format!(
            "output_contract.destination escapes the output base: '{destination}'"
        )));
    }
    Ok(normalized)
}
