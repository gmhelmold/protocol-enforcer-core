//! SPEC_config_layer.md "Macro-level `enabled` — normalize at load", CRITICAL
//! round-trip invariant: normalization (`Profile::with_enabled_macros_only`)
//! is transient/in-memory ONLY. `ProfileManager::save_profile` (and any load
//! path used for editing) must NEVER persist the filtered copy -- a disabled
//! macro must survive a load -> save round-trip unchanged on disk.

use protocol_manifest::ProfileManager;
use protocol_types::profile::{Profile, ProfileSettings, StateDef, SubStateDef, SubStateType};
use tempfile::TempDir;

fn profile_with_disabled_middle_macro(name: &str) -> Profile {
    let checklist = |id: &str| SubStateDef {
        id: format!("{id}_chk"),
        sub_state_type: SubStateType::Checklist,
        name: format!("{id} checklist"),
        description: String::new(),
        enabled: true,
        criteria: Some(vec!["done".to_string()]),
        inject: None,
        verify: None,
        approver_pubkey: None,
        approval_prompt: None,
        hooks: Vec::new(),
    };
    let make = |id: &str, enabled: bool| StateDef {
        state_id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        system_prompt: None,
        enabled,
        max_iterations: 3,
        loop_state: false,
        icon: None,
        sub_states: vec![checklist(id)],
        hooks: Vec::new(),
    };

    Profile {
        name: name.to_string(),
        version: "1.0.0".to_string(),
        description: "round-trip fixture".to_string(),
        protected: false,
        created_at: String::new(),
        cloned_from: None,
        settings: ProfileSettings::default(),
        pipeline: vec![make("a", true), make("b", false), make("c", true)],
        output_contract: None,
    }
}

#[test]
fn disabled_macro_survives_save_load_roundtrip_unchanged_on_disk() {
    let dir = TempDir::new().expect("create temp profiles dir");
    let manager = ProfileManager::new(dir.path());
    let profile = profile_with_disabled_middle_macro("roundtrip-fixture");

    manager.save_profile(&profile).expect("save should succeed");
    let reloaded = manager
        .load_profile("roundtrip-fixture")
        .expect("load should succeed");

    // All 3 macros, in original order, with their original `enabled` flags
    // -- NOT normalized/filtered.
    let ids_and_enabled: Vec<(&str, bool)> = reloaded
        .pipeline
        .iter()
        .map(|m| (m.state_id.as_str(), m.enabled))
        .collect();
    assert_eq!(
        ids_and_enabled,
        vec![("a", true), ("b", false), ("c", true)],
        "save_profile/load_profile must never normalize -- disabled macro 'b' must stay on disk"
    );
}

/// A no-op load -> save round-trip (mirrors the future editor's no-op save)
/// must be byte-for-byte semantically unchanged, in particular never
/// stripping `enabled: false` macros.
#[test]
fn load_then_noop_save_preserves_disabled_macro() {
    let dir = TempDir::new().expect("create temp profiles dir");
    let manager = ProfileManager::new(dir.path());
    let profile = profile_with_disabled_middle_macro("noop-roundtrip");
    manager.save_profile(&profile).expect("initial save");

    let loaded = manager.load_profile("noop-roundtrip").expect("load");
    // Simulate an editor no-op: load, then save straight back (no
    // `with_enabled_macros_only` in between -- that helper must never sit on
    // this path).
    manager.save_profile(&loaded).expect("no-op save");

    let reloaded = manager.load_profile("noop-roundtrip").expect("reload");
    assert_eq!(reloaded.pipeline.len(), 3);
    assert!(
        !reloaded.pipeline[1].enabled,
        "macro 'b' must still be disabled"
    );
}
