// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Protocol FSM crate - core finite state machine (nested profile model).
//! `ProfileFsmEngine` is the sole served-path engine.

pub mod approval;
pub mod config;
pub mod injection;
mod loop_back;
pub mod profile_engine;
pub mod recovery;
pub mod state;
mod transitions;

pub use approval::{APPROVAL_EVIDENCE_KEY, REASON_INVALID, REASON_MISSING};
pub use config::FsmConfig;
pub use profile_engine::{ProfileFsmEngine, RenderedInjection, StepOutcome, StepView};
pub use protocol_artifacts::ArtifactPort;
pub use protocol_ledger::LedgerPort;
pub use state::{SessionState, SessionStatus};
