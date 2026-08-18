// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! The `human_approval` gate: challenge issuance on
//! entry, signature verification on submit.
//!
//! This is the ONE place the enforcer stops being presence-only. Everywhere
//! else a criterion counts as met because a non-null value showed up
//! (passivity: the enforcer never judges quality). Here the enforcer judges
//! nothing either — it checks a signature. That is not an opinion about the
//! work; it is arithmetic over a key the agent does not hold, which is exactly
//! why it is the one thing an agent cannot talk its way past.
//!
//! Two invariants carry the whole gate:
//! 1. The challenge is REPLAYED from the ledger on recovery, never recomputed
//!    (`recovery.rs`), so a restart cannot invalidate a signature a human has
//!    already produced — nor mint a nonce the ledger does not witness.
//! 2. Every entry mints a FRESH challenge, which retires the previous one. A
//!    signature is bound to `(session, macro, sub_state, challenge)`, so it
//!    dies with its challenge: no reuse across loop-backs, macro iterations, or
//!    sibling gates.

use protocol_types::{
    ApprovalChallenge, ChecklistEvidence, CircuitBreakerType, FsmError, FsmEventType, Position,
    SessionStatus, SubStateDef,
};

use crate::profile_engine::StepOutcome;

/// The evidence key an approval submission must carry.
pub const APPROVAL_EVIDENCE_KEY: &str = "approval_signature";

/// Rejection reason: no usable `approval_signature` was submitted at all.
pub const REASON_MISSING: &str = "approval_missing";

/// Rejection reason: a signature was submitted but did not verify against the
/// profile's `approver_pubkey` for THIS gate's live challenge — wrong key,
/// wrong binding, stale challenge, or malformed hex. Deliberately one reason
/// for all four: telling a caller WHICH way its forgery failed is free help it
/// has no honest use for.
pub const REASON_INVALID: &str = "approval_invalid";

/// Mint a fresh challenge for entering `sub` at `position`, or `None` when
/// `sub` is not a `human_approval` sub-state. Callers append the
/// `ApprovalChallengeIssued` event and park it on the session.
pub(crate) fn issue_for(
    sub: &SubStateDef,
    position: &Position,
) -> Result<Option<ApprovalChallenge>, FsmError> {
    if sub.sub_state_type != protocol_types::SubStateType::HumanApproval {
        return Ok(None);
    }
    let challenge = protocol_notary::new_challenge_hex().map_err(FsmError::Internal)?;
    Ok(Some(ApprovalChallenge {
        macro_id: position.macro_id.clone(),
        sub_state_id: position.sub_state_id.clone(),
        challenge,
    }))
}

/// The verdict of checking one approval submission.
pub(crate) enum ApprovalVerdict {
    /// Verified. Carries the signature to record in `ApprovalAccepted`.
    Accepted {
        signature: String,
        challenge: String,
    },
    /// Refused, with the reason to surface and ledger.
    Rejected { reason: &'static str },
}

/// Check `evidence[approval_signature]` against `sub.approver_pubkey` for the
/// session's live challenge. Fail-closed at every step: a gate with no live
/// challenge, no key, or no signature rejects rather than passes.
pub(crate) fn check(
    session_id: &str,
    pos: &Position,
    sub: &SubStateDef,
    pending: Option<&ApprovalChallenge>,
    evidence: &ChecklistEvidence,
) -> ApprovalVerdict {
    let signature = match evidence.get(APPROVAL_EVIDENCE_KEY).and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => {
            return ApprovalVerdict::Rejected {
                reason: REASON_MISSING,
            }
        }
    };

    // No live challenge for THIS position means there is nothing this
    // signature could honestly be bound to (the session was never parked
    // here, or the challenge belongs to a different gate). Reject rather
    // than reach for some other challenge to check against.
    let Some(challenge) =
        pending.filter(|c| c.macro_id == pos.macro_id && c.sub_state_id == pos.sub_state_id)
    else {
        return ApprovalVerdict::Rejected {
            reason: REASON_INVALID,
        };
    };

    // `validate_profile` guarantees this key exists and parses, so a
    // profile that reached the engine cannot land here — but a gate must never
    // pass on an absent key, so the fallback is a rejection, not an unwrap.
    let Some(pubkey) = sub.approver_pubkey.as_deref() else {
        return ApprovalVerdict::Rejected {
            reason: REASON_INVALID,
        };
    };

    if protocol_notary::verify_approval_hex(
        pubkey,
        session_id,
        &pos.macro_id,
        &pos.sub_state_id,
        &challenge.challenge,
        &signature,
    ) {
        ApprovalVerdict::Accepted {
            signature,
            challenge: challenge.challenge.clone(),
        }
    } else {
        ApprovalVerdict::Rejected {
            reason: REASON_INVALID,
        }
    }
}

/// The stepping half of the gate, an inherent method on `ProfileFsmEngine`
/// (same pattern as `transitions.rs`): it lives here, next to the rules it
/// enforces, so the gate's decision and its ledger record cannot drift apart.
impl<L: protocol_ledger::LedgerPort> crate::profile_engine::ProfileFsmEngine<L> {
    /// Called when the current sub is a `human_approval` gate.
    /// Verify the submitted `approval_signature` against the profile's
    /// `approver_pubkey` for the challenge issued when the session entered this
    /// gate; advance exactly like a plain sub-state on success, refuse with a
    /// typed reason otherwise. The refusal is position-neutral (the session
    /// stays on the gate) and leaves the SAME challenge live — the human's
    /// signature is not invalidated by an agent fumbling the submission.
    pub(crate) fn step_approval(
        &mut self,
        session_id: &str,
        macro_idx: usize,
        sub_idx: usize,
        pos: &Position,
        evidence: ChecklistEvidence,
    ) -> Result<StepOutcome, FsmError> {
        let sub = &self.profile.pipeline[macro_idx].sub_states[sub_idx];
        let pending = self
            .sessions
            .get(session_id)
            .and_then(|s| s.pending_approval.clone());
        let verdict = check(session_id, pos, sub, pending.as_ref(), &evidence);

        match verdict {
            ApprovalVerdict::Rejected { reason } => {
                self.append(
                    session_id,
                    FsmEventType::ApprovalRejected {
                        macro_id: pos.macro_id.clone(),
                        sub_state_id: pos.sub_state_id.clone(),
                        reason: reason.to_string(),
                    },
                    pos,
                    serde_json::json!({}),
                )?;

                // issue #70 (security-audit FIX 3):
                // bound the reject path. Without this an agent can spam garbage
                // signatures forever — each one a fresh `ApprovalRejected`
                // append — growing the ledger without limit while the same
                // challenge stays live. Count consecutive rejections at THIS
                // gate entry (reset when the challenge is minted, in
                // `enter_sub`, and by a valid signature's advance); at the cap,
                // trip a typed circuit breaker instead of appending forever. A
                // VALID signature is unaffected — it advances before reaching
                // here.
                let cap = self.profile.settings.approval_rejection_cap;
                let count = {
                    let session = self
                        .sessions
                        .get_mut(session_id)
                        .ok_or_else(|| FsmError::SessionNotFound(session_id.to_string()))?;
                    session.consecutive_approval_rejections += 1;
                    session.consecutive_approval_rejections
                };
                if cap != 0 && count >= cap {
                    let breaker = CircuitBreakerType::ApprovalRejectionLimit;
                    self.append(
                        session_id,
                        FsmEventType::CircuitBreakerTriggered {
                            breaker: breaker.clone(),
                            details: serde_json::json!({
                                "macro": pos.macro_id,
                                "sub_state": pos.sub_state_id,
                                "consecutive_approval_rejections": count,
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
                            "Approval rejection cap ({cap}) exceeded: {count} consecutive rejected submissions"
                        ),
                    });
                }

                Ok(StepOutcome::Rejected {
                    rejected_items: vec![APPROVAL_EVIDENCE_KEY.to_string()],
                    reason: reason.to_string(),
                    position: pos.clone(),
                })
            }
            ApprovalVerdict::Accepted {
                signature,
                challenge,
            } => {
                // Ledgered BEFORE the advance, so the accepted event sits with
                // the gate it passed and an auditor can re-verify the approval
                // offline from the ledger alone.
                self.append(
                    session_id,
                    FsmEventType::ApprovalAccepted {
                        macro_id: pos.macro_id.clone(),
                        sub_state_id: pos.sub_state_id.clone(),
                        challenge,
                        signature,
                    },
                    pos,
                    serde_json::json!({}),
                )?;
                self.advance_plain(session_id, macro_idx, sub_idx, pos)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol_types::SubStateType;
    use std::collections::HashMap;

    const SEED: &str = "0707070707070707070707070707070707070707070707070707070707070707";

    fn sub(kind: SubStateType, pubkey: Option<&str>) -> SubStateDef {
        SubStateDef {
            id: "gate".to_string(),
            sub_state_type: kind,
            name: "Gate".to_string(),
            description: String::new(),
            enabled: true,
            criteria: None,
            inject: None,
            verify: None,
            approver_pubkey: pubkey.map(str::to_string),
            approval_prompt: None,
            hooks: Vec::new(),
        }
    }

    fn pos() -> Position {
        Position {
            macro_id: "approve".to_string(),
            sub_state_id: "gate".to_string(),
        }
    }

    fn evidence(sig: Option<&str>) -> ChecklistEvidence {
        let mut m: ChecklistEvidence = HashMap::new();
        if let Some(s) = sig {
            m.insert(
                APPROVAL_EVIDENCE_KEY.to_string(),
                serde_json::json!(s.to_string()),
            );
        }
        m
    }

    fn pending(challenge: &str) -> ApprovalChallenge {
        ApprovalChallenge {
            macro_id: "approve".to_string(),
            sub_state_id: "gate".to_string(),
            challenge: challenge.to_string(),
        }
    }

    #[test]
    fn issue_only_fires_for_human_approval() {
        assert!(issue_for(&sub(SubStateType::Execute, None), &pos())
            .unwrap()
            .is_none());
        let issued = issue_for(&sub(SubStateType::HumanApproval, None), &pos())
            .unwrap()
            .unwrap();
        assert_eq!(issued.macro_id, "approve");
        assert_eq!(issued.sub_state_id, "gate");
        assert_eq!(issued.challenge.len(), 64);
    }

    #[test]
    fn correct_signature_is_accepted() {
        let pk = protocol_notary::pubkey_hex_from_seed_hex(SEED).unwrap();
        let sig = protocol_notary::sign_approval_hex(SEED, "s1", "approve", "gate", "ab").unwrap();
        let verdict = check(
            "s1",
            &pos(),
            &sub(SubStateType::HumanApproval, Some(&pk)),
            Some(&pending("ab")),
            &evidence(Some(&sig)),
        );
        assert!(matches!(verdict, ApprovalVerdict::Accepted { .. }));
    }

    #[test]
    fn absent_or_blank_signature_is_missing_not_invalid() {
        let pk = protocol_notary::pubkey_hex_from_seed_hex(SEED).unwrap();
        let s = sub(SubStateType::HumanApproval, Some(&pk));
        for ev in [evidence(None), evidence(Some("   "))] {
            match check("s1", &pos(), &s, Some(&pending("ab")), &ev) {
                ApprovalVerdict::Rejected { reason } => assert_eq!(reason, REASON_MISSING),
                _ => panic!("expected a missing-signature rejection"),
            }
        }
    }

    #[test]
    fn no_live_challenge_rejects() {
        let pk = protocol_notary::pubkey_hex_from_seed_hex(SEED).unwrap();
        let sig = protocol_notary::sign_approval_hex(SEED, "s1", "approve", "gate", "ab").unwrap();
        let s = sub(SubStateType::HumanApproval, Some(&pk));
        // Nothing pending at all.
        assert!(matches!(
            check("s1", &pos(), &s, None, &evidence(Some(&sig))),
            ApprovalVerdict::Rejected {
                reason: REASON_INVALID
            }
        ));
        // Pending, but for a DIFFERENT gate — must not be borrowed for this one.
        let elsewhere = ApprovalChallenge {
            macro_id: "other".to_string(),
            sub_state_id: "gate".to_string(),
            challenge: "ab".to_string(),
        };
        assert!(matches!(
            check("s1", &pos(), &s, Some(&elsewhere), &evidence(Some(&sig))),
            ApprovalVerdict::Rejected {
                reason: REASON_INVALID
            }
        ));
    }

    #[test]
    fn missing_key_rejects_rather_than_passes() {
        let sig = protocol_notary::sign_approval_hex(SEED, "s1", "approve", "gate", "ab").unwrap();
        assert!(matches!(
            check(
                "s1",
                &pos(),
                &sub(SubStateType::HumanApproval, None),
                Some(&pending("ab")),
                &evidence(Some(&sig)),
            ),
            ApprovalVerdict::Rejected {
                reason: REASON_INVALID
            }
        ));
    }
}
