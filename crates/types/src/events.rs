// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! FSM Event definitions

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsmEvent {
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub event_type: FsmEventType,
    pub step_id: Option<String>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FsmEventType {
    SessionStarted {
        pipeline_id: String,
        manifest_version: String,
        /// SHA-256 (lowercase hex) of the profile file bytes as loaded, so the
        /// transcript root commits to the protocol in force
        /// (SPEC_transcript_notary.md §5). Additive: `default` +
        /// `skip_serializing_if` keep pre-feature ledgers deserializable and a
        /// hash-less event byte-identical on the wire (NOT-12/NOT-13).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_sha256: Option<String>,
    },
    MilestoneSubmitted {
        evidence_keys: Vec<String>,
        iteration: u32,
    },
    MilestoneAccepted {
        next_step_id: Option<String>,
    },
    MilestoneRejected {
        rejected_items: Vec<String>,
        reason: String,
        iteration: u32,
    },
    StepAdvanced {
        from_step: String,
        to_step: String,
    },
    /// Sub-state transition within a macro-state (SPEC_v3 §1.1). Emitted by
    /// `ProfileFsmEngine` on a plain (non-checklist) sub-state advance.
    SubStateAdvanced {
        from_sub: String,
        to_sub: String,
    },
    /// Macro-state transition in the canonical pipeline (SPEC_v3 §1.1).
    /// Emitted by `ProfileFsmEngine` when a macro's checklist passes and the
    /// pipeline advances to the next macro.
    MacroAdvanced {
        from_macro: String,
        to_macro: String,
    },
    CircuitBreakerTriggered {
        breaker: CircuitBreakerType,
        details: serde_json::Value,
    },
    SessionCompleted {
        final_artifact_path: String,
        /// SHA-256 (lowercase hex) of the exact output-contract bytes written
        /// at completion (SPEC_transcript_notary.md §8), so the session's
        /// primary deliverable is committed by the root, not referenced by path
        /// alone. `None` when the profile declares no output contract. Additive
        /// on the wire, same as `profile_sha256`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_sha256: Option<String>,
    },
    SessionFailed {
        reason: String,
        final_step: String,
    },
    /// A checklist rejection at macro `M` looped back to `M`'s first enabled
    /// Execute sub-state (SPEC_loopback.md), instead of leaving the session
    /// on the checklist. `iteration` is `macro_iteration` AFTER this
    /// rejection's increment (mirrors `MilestoneRejected.iteration`); it is
    /// NOT reset by the loop-back. Anchored (via `FsmEvent.step_id`) at the
    /// ARRIVAL position (`to_sub`), not the checklist -- see
    /// `ProfileFsmEngine::reject_checklist`.
    MacroLoopedBack {
        from_sub: String,
        to_sub: String,
        iteration: u32,
    },
    /// An artifact was offloaded through the enforcer's own `ArtifactStore`
    /// (SPEC_v3 §9, FR-25). The `sha256` in the ref is ENFORCER-authored (the
    /// store hashes the bytes it received), so replaying this event rehydrates
    /// `SessionState.artifacts` exactly, and the completion-time integrity
    /// check re-verifies the ref against the still-on-disk blob.
    ArtifactStored {
        artifact_ref: crate::common::ArtifactRef,
    },
    /// A `human_approval` sub-state was ENTERED and the engine minted a fresh
    /// 32-byte challenge nonce for it (SPEC_human_approval.md). Ledgering the
    /// nonce is what makes it survivable: recovery REPLAYS this event rather
    /// than regenerating a nonce, so a crash cannot invalidate a signature the
    /// human already produced. Re-entry (loop-back, a later macro iteration)
    /// emits a NEW event with a NEW nonce, which is exactly what retires the
    /// previous one.
    ApprovalChallengeIssued {
        macro_id: String,
        sub_state_id: String,
        challenge: String,
    },
    /// A `human_approval` gate was passed: `signature` is the hex Ed25519
    /// signature that verified against the profile's `approver_pubkey` over the
    /// canonical message for `challenge`. Recorded so an auditor can
    /// re-verify the approval OFFLINE from the ledger alone — the transcript
    /// root commits to this line, so the approval is as tamper-evident as every
    /// other step.
    ApprovalAccepted {
        macro_id: String,
        sub_state_id: String,
        challenge: String,
        signature: String,
    },
    /// A `human_approval` submission was refused (`approval_missing` /
    /// `approval_invalid`). Position- and counter-neutral by construction: it
    /// records the attempt for audit without moving the session, so the live
    /// engine and a recovered one agree with no extra bookkeeping.
    ApprovalRejected {
        macro_id: String,
        sub_state_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CircuitBreakerType {
    MaxIterations,
    RepetitiveLoop,
    GlobalTimeout,
    /// Too many consecutive rejected `human_approval` submissions at one gate
    /// entry (security-audit FIX 3, issue #70). Bounds the `ApprovalRejected`
    /// ledger growth an attacker could otherwise drive without limit by
    /// spamming garbage signatures; a VALID signature still advances.
    ApprovalRejectionLimit,
}
