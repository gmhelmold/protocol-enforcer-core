// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Crash-recovery cache-miss helper (SPEC_crash_recovery.md).
//!
//! `ProtocolServer` keeps one `ProfileFsmEngine` per session in an
//! in-memory map. When a session is missing from that map (process
//! restart, cache eviction, a different worker), `recover_into_cache`
//! rebuilds it from the on-disk ledger via the sidecar `protocol_start`
//! wrote (`server::write_sidecar_atomic`), so `protocol_submit_milestone`
//! and `protocol_get_state` don't unconditionally 404 a session that is
//! really still alive on disk.
//!
//! Passivity (SPEC_crash_recovery.md "Passivity"): this module re-loads the
//! profile and revalidates it through the exact same strict
//! `validate_profile` call `protocol_start` uses -- the identical served
//! path. It never resolves, reads, or learns about hooks.

use std::path::Path;

use rmcp::ErrorData;

use protocol_fsm::{FsmConfig, ProfileFsmEngine};
use protocol_ledger::Ledger;
use protocol_library::Library;

use crate::error_map::{fsm_error_to_mcp, manifest_invalid_error};
use crate::paths::{ledger_dir, sidecar_path};
use crate::wire::profile_sha256_of;

#[derive(serde::Deserialize)]
struct Sidecar {
    manifest_path: String,
}

/// Rebuild `session_id`'s `ProfileFsmEngine` from its on-disk ledger + the
/// sidecar `protocol_start` wrote. Returns the SAME `SESSION_NOT_FOUND`
/// error the caller uses today when the sidecar is absent (a genuinely
/// unknown session), so cache-miss recovery never invents sessions. Any
/// other failure (unreadable sidecar, profile no longer loadable/valid,
/// ledger/replay failure) fails loud -- never a silent or partial serve.
pub(crate) fn recover_into_cache(session_id: &str) -> Result<ProfileFsmEngine<Ledger>, ErrorData> {
    let sidecar_bytes = std::fs::read(sidecar_path(session_id)).map_err(|_| {
        fsm_error_to_mcp(protocol_types::FsmError::SessionNotFound(
            session_id.to_string(),
        ))
    })?;
    let sidecar: Sidecar = serde_json::from_slice(&sidecar_bytes).map_err(|e| {
        ErrorData::internal_error(format!("Corrupt crash-recovery sidecar: {}", e), None)
    })?;

    let profile = protocol_manifest::load_profile(Path::new(&sidecar.manifest_path))
        .map_err(|e| ErrorData::invalid_params(format!("Failed to load profile: {}", e), None))?;
    let library = Library::from_env();
    if let Err(violations) = protocol_manifest::validate_profile(&profile, &library) {
        return Err(manifest_invalid_error(violations));
    }

    // `Ledger::new` uses OpenOptions create+append -- it ATTACHES to the
    // existing on-disk ledger file for this session_id, never truncating,
    // which is what makes replay-based recovery safe here.
    let ledger = Ledger::new(session_id, &ledger_dir())
        .map_err(|e| ErrorData::internal_error(format!("Ledger init failed: {}", e), None))?;

    let mut config = FsmConfig::from_settings(&profile.settings);
    config.artifact_root = crate::paths::artifact_root();
    // Config-parity with the start path (§5): hash the same file the sidecar
    // recorded so a recovered engine's config matches the original's. NOTE: this
    // has no observable ledger effect on recovery — `recover_session` REPLAYS the
    // existing events (which already carry the original `profile_sha256`) and does
    // NOT re-emit `SessionStarted`, the only event that reads this field. Kept for
    // parity and forward-safety, not because recovery emits it.
    config.profile_sha256 = profile_sha256_of(Path::new(&sidecar.manifest_path));
    let mut engine = ProfileFsmEngine::new(profile, library, ledger, config);
    engine
        .recover_session(session_id)
        .map_err(fsm_error_to_mcp)?;
    Ok(engine)
}
