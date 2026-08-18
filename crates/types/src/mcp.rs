// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! MCP Protocol types - wire format definitions
//!
//! These are the MCP protocol DTOs for the gateway transport layer.
//! Not yet wired into any handlers.

use crate::{ArtifactRef, ChecklistEvidence, FsmEvent, StepInfo};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolStartParams {
    pub manifest_path: String,
    pub session_id: Option<String>,
    pub initial_context: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolStartResult {
    pub session_id: String,
    pub current_step: StepInfo,
    pub manifest_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolSubmitParams {
    pub session_id: String,
    pub step_id: String,
    pub checklist_evidence: ChecklistEvidence,
    pub artifacts: std::collections::HashMap<String, ArtifactRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "advanced")]
pub enum ProtocolSubmitResult {
    #[serde(rename = "true")]
    Advanced {
        next_step: Option<StepInfo>,
        session_state_json: serde_json::Value,
    },
    #[serde(rename = "false")]
    Rejected {
        rejected_items: Vec<String>,
        reason: String,
        current_step: StepInfo,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolGetStateParams {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolGetStateResult {
    pub session_state_json: serde_json::Value,
    pub ledger_tail: Vec<FsmEvent>,
}
