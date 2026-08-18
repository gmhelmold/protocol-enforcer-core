// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Hook-inert advisory, split out of `server.rs::protocol_start` to keep
//! that file under the LOC cap enforced elsewhere in this workspace --
//! mirrors `crates/fsm/src/loop_back.rs`, which is split out for the same
//! reason.

use protocol_types::Profile;

/// The passivity invariant: `protocol-gateway` NEVER executes hooks --
/// only `protocol-orchestrator` does. A profile that declares hooks still
/// loads and serves identically (this is advisory-only and changes
/// nothing about how the profile is handled), but a hook-bearing profile
/// served on this path would silently look enforced while its hooks never
/// run. Warn once, at load time, so that's visible. Called from
/// `ProtocolServer::protocol_start`'s profile-loading closure, right after
/// `validate_profile` succeeds.
pub(crate) fn warn_if_hooks_inert(profile: &Profile, session_id: &str) {
    let has_hooks = profile.pipeline.iter().any(|state| {
        !state.hooks.is_empty() || state.sub_states.iter().any(|sub| !sub.hooks.is_empty())
    });
    if has_hooks {
        tracing::warn!(
            session_id = %session_id,
            profile = %profile.name,
            "profile declares hooks, but protocol-gateway is a passive path and never \
             executes hooks -- they are advisory/inert here"
        );
    }
}
