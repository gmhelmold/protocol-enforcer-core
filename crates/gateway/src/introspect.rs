// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Read-only profile introspection (SPEC_mcp_introspection.md).
//!
//! Backs the two additive, non-mutating MCP tools `protocol_profile_list`
//! and `protocol_profile_show`. This module is deliberately **hook-blind**:
//! it never takes a `protocol_library::Library`, never resolves a hook, and
//! never emits a `hooks` field -- it serializes only the macro/sub-state
//! structure the enforcer itself acts on (SPEC §"Passivity"). All listing
//! and name-safety logic is delegated to `protocol_manifest::ProfileManager`
//! (`list_profiles`/`load_profile`, which internally guards `is_safe_name`)
//! rather than reimplemented here, so a `../`-style name is rejected by the
//! same guard the CLI/library already trust -- never a filesystem escape.

use std::path::PathBuf;

use rmcp::ErrorData;

use protocol_manifest::ProfileManager;
use protocol_types::profile::{Profile, SubStateDef};

/// The profiles directory this gateway serves from: `PROTOCOL_PROFILES_DIR`
/// if set, else `./profiles` -- matching the CLI/library convention
/// (SPEC_mcp_introspection.md "Profiles directory").
pub(crate) fn profiles_dir() -> PathBuf {
    std::env::var("PROTOCOL_PROFILES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./profiles"))
}

/// `protocol_profile_list` body: `{ "profiles": [name, ...] }`.
pub(crate) fn list() -> Result<serde_json::Value, ErrorData> {
    let manager = ProfileManager::new(profiles_dir());
    Ok(serde_json::json!({ "profiles": manager.list_profiles() }))
}

/// `protocol_profile_show` body: loads `name` via `ProfileManager` (which
/// guards `is_safe_name` and existence internally, so an unsafe or missing
/// name never reaches the filesystem as a raw path) and returns the
/// hook-blind structural view.
pub(crate) fn show(name: &str) -> Result<serde_json::Value, ErrorData> {
    let manager = ProfileManager::new(profiles_dir());
    let profile = manager.load_profile(name).map_err(|e| {
        ErrorData::invalid_params(
            format!("Profile not found or invalid: {}", e),
            Some(serde_json::json!({ "code": "PROFILE_NOT_FOUND", "name": name })),
        )
    })?;
    Ok(profile_json(&profile))
}

/// Hook-blind structural serializer (SPEC_mcp_introspection.md): the exact
/// shape `{ name, version, description, pipeline: [ { state_id, name,
/// enabled, sub_states: [ { id, name, type, criteria? } ] } ] }`. Deliberately
/// does NOT take a `Library` and never reads/emits `.hooks` -- unlike the CLI
/// `print_tree`, which resolves hook kinds against a `Library`.
fn profile_json(profile: &Profile) -> serde_json::Value {
    let pipeline: Vec<serde_json::Value> = profile
        .pipeline
        .iter()
        .map(|state| {
            let sub_states: Vec<serde_json::Value> =
                state.sub_states.iter().map(sub_state_json).collect();
            serde_json::json!({
                "state_id": state.state_id,
                "name": state.name,
                "enabled": state.enabled,
                "sub_states": sub_states,
            })
        })
        .collect();

    serde_json::json!({
        "name": profile.name,
        "version": profile.version,
        "description": profile.description,
        "pipeline": pipeline,
    })
}

/// One `sub_states[]` entry. `criteria` is only present (as a JSON key) when
/// `Some` -- absent (not `null`) otherwise, matching the spec's `criteria?`.
fn sub_state_json(sub: &SubStateDef) -> serde_json::Value {
    let mut value = serde_json::json!({
        "id": sub.id,
        "name": sub.name,
        "type": sub.sub_state_type,
    });
    if let Some(criteria) = &sub.criteria {
        value["criteria"] = serde_json::json!(criteria);
    }
    value
}
