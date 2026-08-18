// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Common type definitions

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub type ChecklistEvidence = HashMap<String, serde_json::Value>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepInfo {
    pub step_id: String,
    pub name: String,
    pub exit_checklist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputContract {
    pub format: String,
    pub schema: serde_json::Value,
    pub destination: String,
}

/// Runtime session state for the nested (macro/sub-state) model
/// (SPEC_v3 §1.1 end-state). The flat engine's `current_step`/`iteration`/
/// `context` fields were retired in WP6 along with the flat served path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub pipeline_id: String,
    /// Current macro-state/sub-state position. `None` when
    /// Completed/Failed.
    pub position: Option<Position>,
    pub status: SessionStatus,
    /// Checklist retries scoped to the current macro-state.
    pub macro_iteration: u32,
    pub consecutive_identical_rejections: u32,
    pub last_rejected_evidence_keys: Vec<String>,
    /// Consecutive `human_approval` rejections at the CURRENT gate entry
    /// (security-audit FIX 3, issue #70). Reset to 0 every time a fresh
    /// challenge is minted for the gate (entry / loop-back / re-iteration) and
    /// on a valid signature; incremented on each rejected submission. When it
    /// reaches `ProfileSettings::approval_rejection_cap` the engine trips an
    /// `ApprovalRejectionLimit` circuit breaker instead of appending to the
    /// ledger without bound. `#[serde(default)]` so older persisted sessions
    /// (which predate this field) rehydrate at 0.
    #[serde(default)]
    pub consecutive_approval_rejections: u32,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Snapshot of the context supplied when the session started.
    pub initial_context: serde_json::Value,
    #[serde(default)]
    pub artifacts: HashMap<String, ArtifactRef>,
    /// The live challenge for the `human_approval` sub-state this session is
    /// parked on, or `None` anywhere else (SPEC_human_approval.md). Set when
    /// the engine ENTERS such a sub-state and cleared when it leaves; on
    /// recovery it is replayed from the `ApprovalChallengeIssued` ledger event,
    /// never regenerated — a fresh nonce after a crash would invalidate a
    /// signature a human had already produced.
    #[serde(default)]
    pub pending_approval: Option<ApprovalChallenge>,
}

/// A challenge nonce bound to one entry into one `human_approval` sub-state
/// (SPEC_human_approval.md). `challenge` is 32 random bytes in lowercase hex;
/// the position fields pin the signature to this exact gate, so a signature
/// harvested for one gate cannot be replayed at another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalChallenge {
    pub macro_id: String,
    pub sub_state_id: String,
    pub challenge: String,
}

/// A position within the canonical macro-state/sub-state pipeline
/// (SPEC_v3 §1.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub macro_id: String,
    pub sub_state_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionStatus {
    Active,
    Completed,
    Failed,
}
