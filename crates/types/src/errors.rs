// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Error type definitions

use crate::events::CircuitBreakerType;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("YAML parse error: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error("Schema file not found: {0}")]
    SchemaNotFound(std::path::PathBuf),
    #[error("Invalid JSON Schema: {0}")]
    SchemaInvalid(String),
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Duplicate step_id: {0}")]
    DuplicateStepId(String),
    #[error("Empty exit_checklist for step: {0}")]
    EmptyChecklist(String),
    #[error("Step order non-lexical: {0} before {1}")]
    StepOrder(String, String),
    #[error("Missing schema_path in output_contract")]
    MissingSchemaPath,
    #[error("Destination missing {{session_id}} placeholder")]
    MissingSessionIdPlaceholder,
}

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Corrupt ledger at line {line}: {message}")]
    Corrupt { line: usize, message: String },
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
}

#[derive(Debug, Error)]
pub enum FsmError {
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    /// The session exists (tracked in-memory or has a ledger) but is no
    /// longer `Active` (already `Completed` or `Failed`) -- distinct from
    /// `SessionNotFound`, which means no such session was ever known here.
    #[error("Session '{session_id}' is not active (status: {status:?})")]
    SessionInactive {
        session_id: String,
        status: crate::common::SessionStatus,
    },
    #[error("Step mismatch: expected {expected}, got {actual}")]
    StepMismatch { expected: String, actual: String },
    #[error("Checklist incomplete: missing {items:?}")]
    ChecklistIncomplete { items: Vec<String> },
    #[error("Circuit breaker triggered: {breaker:?} - {details}")]
    CircuitBreaker {
        breaker: CircuitBreakerType,
        details: String,
    },
    #[error("Output contract violation: {0}")]
    OutputContractViolation(String),
    #[error("Ledger error: {0}")]
    Ledger(#[from] LedgerError),
    #[error("Artifact error: {0}")]
    Artifact(#[from] ArtifactError),
    #[error("Library reference unresolved: {0}")]
    LibraryUnresolved(String),
    #[error("Manifest invalid: {0}")]
    ManifestInvalid(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Error)]
pub enum ContractError {
    #[error("Validation failed: {0}")]
    ValidationFailed(String),
    #[error("Schema error: {0}")]
    SchemaError(String),
}

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("FSM error: {0}")]
    Fsm(#[from] FsmError),
    #[error("Transport error: {0}")]
    Transport(String),
}
