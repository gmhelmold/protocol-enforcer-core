// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Shared transcript-root construction — the single realisation that both the
//! offline verifier (`notarize`) and the out-of-band witness signer
//! (`notary-sign`) call, so producer and verifier can never drift.
//!
//! The gateway's completion path (`gateway::wire`) computes the same value from
//! the same primitives (`merkle::root` over `Ledger::lines`); it keeps its own
//! copy to avoid a gateway→ledger-helper churn, and the two are proven identical
//! by the notary tests. This module exists so the *signing* pair shares one
//! builder — a signature is only meaningful if signer and verifier agree byte
//! for byte on what was signed.

use std::path::Path;

use protocol_types::{FsmEventType, LedgerError};

use crate::{merkle, Ledger};

/// Lowercase-hex encode. Kept here (not pulled from a hex crate) so the ledger
/// crate stays dependency-light and every root-hex string in the system has one
/// definition.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// The RFC 6962 transcript root as a lowercase-hex string, over the session's
/// ledger leaves. `Err` when the ledger file is absent (NOT-4: a missing ledger
/// is an error, never an empty tree) — see [`Ledger::lines`].
pub fn transcript_root_hex(session_id: &str, dir: &Path) -> Result<String, LedgerError> {
    let leaves = Ledger::lines(session_id, dir)?;
    Ok(to_hex(&merkle::root(&leaves)))
}

/// The raw 32-byte transcript root (what a signature is actually taken over).
pub fn transcript_root_bytes(session_id: &str, dir: &Path) -> Result<[u8; 32], LedgerError> {
    let leaves = Ledger::lines(session_id, dir)?;
    Ok(merkle::root(&leaves))
}

/// The `profile_sha256` the root commits to, read from the `SessionStarted`
/// event. `None` for a pre-WP-2 ledger that never recorded it. Read via `replay`
/// so signer and verifier resolve it identically.
pub fn profile_sha256(session_id: &str, dir: &Path) -> Result<Option<String>, LedgerError> {
    let events = Ledger::replay(session_id, dir)?;
    Ok(events.iter().find_map(|e| match &e.event_type {
        FsmEventType::SessionStarted {
            profile_sha256: Some(sha),
            ..
        } => Some(sha.clone()),
        _ => None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merkle;

    #[test]
    fn hex_roundtrip_shape() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xff]), "000fff");
        assert_eq!(to_hex(&[]), "");
    }

    #[test]
    fn root_hex_matches_manual_merkle() {
        let dir = tempfile::tempdir().unwrap();
        let sid = "s";
        std::fs::write(dir.path().join("s.jsonl"), b"a\nb\nc\n").unwrap();
        let expect = to_hex(&merkle::root(&Ledger::lines(sid, dir.path()).unwrap()));
        assert_eq!(transcript_root_hex(sid, dir.path()).unwrap(), expect);
        // and the byte form agrees with the hex form.
        assert_eq!(
            to_hex(&transcript_root_bytes(sid, dir.path()).unwrap()),
            expect
        );
    }

    #[test]
    fn missing_ledger_is_err_not_empty_root() {
        let dir = tempfile::tempdir().unwrap();
        assert!(transcript_root_hex("nope", dir.path()).is_err());
    }
}
