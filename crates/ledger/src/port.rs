// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! LedgerPort trait definition

use protocol_types::{FsmEvent, LedgerError};

pub trait LedgerPort: Send + Sync {
    /// Append one event. Takes `&self`: implementations use interior mutability
    /// (the concrete `Ledger` wraps the file in `Arc<Mutex<File>>`), so a single
    /// ledger shared across threads (`Arc<dyn LedgerPort>`) appends safely with
    /// no external lock — the write is serialized internally.
    fn append(&self, event: &FsmEvent) -> Result<(), LedgerError>;
    fn replay(&self, session_id: &str) -> Result<Vec<FsmEvent>, LedgerError>;
}
