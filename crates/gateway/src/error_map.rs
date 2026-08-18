// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Error mapping: `FsmError`/`GatewayError` -> typed MCP `ErrorData` (SPEC_v3 §7).
//!
//! Every mapped error carries a `data.code` string matching the SPEC §7
//! error list (`MANIFEST_INVALID`, `SESSION_NOT_FOUND`, `SESSION_INACTIVE`,
//! `STEP_MISMATCH`, `CHECKLIST_INCOMPLETE`, `CIRCUIT_BREAKER`,
//! `OUTPUT_CONTRACT_VIOLATION`, `LEDGER_CORRUPT`, `LIBRARY_UNRESOLVED`,
//! `INTERNAL_ERROR`) plus whatever structured detail is available, so MCP
//! clients can branch on `data.code` instead of parsing `message`. This
//! module only serializes/maps errors produced elsewhere -- it contains no
//! gate/breaker decision logic itself.

use protocol_types::{FsmError, GatewayError};
use rmcp::ErrorData;

/// Map an `FsmError` straight to a typed MCP `ErrorData`.
pub fn fsm_error_to_mcp(err: FsmError) -> ErrorData {
    match err {
        FsmError::SessionNotFound(id) => ErrorData::invalid_params(
            format!("Session not found: {}", id),
            Some(serde_json::json!({ "code": "SESSION_NOT_FOUND", "session_id": id })),
        ),
        FsmError::SessionInactive { session_id, status } => ErrorData::invalid_params(
            format!(
                "Session '{}' is not active (status: {:?})",
                session_id, status
            ),
            Some(serde_json::json!({
                "code": "SESSION_INACTIVE",
                "session_id": session_id,
                "status": status,
            })),
        ),
        FsmError::StepMismatch { expected, actual } => ErrorData::invalid_params(
            format!("Step mismatch: expected {}, got {}", expected, actual),
            Some(serde_json::json!({
                "code": "STEP_MISMATCH",
                "expected": expected,
                "actual": actual,
            })),
        ),
        FsmError::ChecklistIncomplete { items } => ErrorData::invalid_params(
            format!("Checklist incomplete: {:?}", items),
            Some(serde_json::json!({ "code": "CHECKLIST_INCOMPLETE", "items": items })),
        ),
        FsmError::CircuitBreaker { breaker, details } => ErrorData::invalid_params(
            format!("Circuit breaker {:?}: {}", breaker, details),
            Some(serde_json::json!({
                "code": "CIRCUIT_BREAKER",
                "breaker": breaker,
                "details": details,
            })),
        ),
        FsmError::OutputContractViolation(msg) => ErrorData::invalid_params(
            format!("Output contract violation: {}", msg),
            Some(serde_json::json!({
                "code": "OUTPUT_CONTRACT_VIOLATION",
                "errors": [msg],
            })),
        ),
        FsmError::Ledger(e) => ErrorData::internal_error(
            format!("Ledger error: {}", e),
            Some(serde_json::json!({ "code": "LEDGER_CORRUPT", "message": e.to_string() })),
        ),
        FsmError::Artifact(e) => ErrorData::internal_error(
            format!("Artifact error: {}", e),
            Some(serde_json::json!({ "code": "INTERNAL_ERROR", "message": e.to_string() })),
        ),
        FsmError::LibraryUnresolved(msg) => ErrorData::invalid_params(
            format!("Library unresolved: {}", msg),
            Some(serde_json::json!({ "code": "LIBRARY_UNRESOLVED", "message": msg })),
        ),
        FsmError::ManifestInvalid(msg) => {
            manifest_invalid_error(msg.split("; ").map(str::to_string).collect())
        }
        FsmError::Internal(msg) => ErrorData::internal_error(
            format!("Internal error: {}", msg),
            Some(serde_json::json!({ "code": "INTERNAL_ERROR", "message": msg })),
        ),
    }
}

/// Typed `MANIFEST_INVALID` error carrying the full violations list
/// (used by `protocol_start`'s explicit pre-flight `validate_profile` call).
pub fn manifest_invalid_error(violations: Vec<String>) -> ErrorData {
    ErrorData::invalid_params(
        format!("Manifest invalid: {}", violations.join("; ")),
        Some(serde_json::json!({ "code": "MANIFEST_INVALID", "violations": violations })),
    )
}

/// Map a `GatewayError` to an MCP `ErrorData` suitable for returning from a
/// tool handler (surfaced to the connected MCP client as a JSON-RPC error).
pub fn gateway_error_to_mcp(err: GatewayError) -> ErrorData {
    match err {
        GatewayError::SessionNotFound(id) => fsm_error_to_mcp(FsmError::SessionNotFound(id)),
        GatewayError::InvalidRequest(msg) => ErrorData::invalid_params(msg, None),
        GatewayError::Fsm(fsm_err) => fsm_error_to_mcp(fsm_err),
        GatewayError::Transport(msg) => ErrorData::internal_error(msg, None),
    }
}
