// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Acceptance tests for `ProfileEditor` (SPEC_config_layer.md WP-A), part 2:
//! macro/sub-state add-remove-move, name_collision, rename/conflict, and the
//! protected/not-found/unsafe-name guards + remaining profile-level verbs.
//! Items 1-4 live in `profile_editor_tests.rs` -- split to stay under the
//! workspace's 600-line-per-file cap. (The `EditError::Conflict`
//! compare-and-swap simulation itself lives as an in-module unit test in
//! `src/editor/mod.rs`, since it needs the private `apply` closure
//! primitive.)
//!
//! Each integration test binary is compiled as its own crate, so the small
//! fixture helpers below are duplicated from `profile_editor_tests.rs`
//! rather than shared.

use protocol_library::Library;
use protocol_manifest::{EditError, ProfileEditor, ProfileManager};
use protocol_types::profile::{Profile, ProfileSettings, StateDef, SubStateDef, SubStateType};
use std::path::PathBuf;
use tempfile::TempDir;

fn fixture_library() -> Library {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library");
    Library::new(root)
}

/// A base profile: one macro `phase1` with inject -> execute -> checklist.
fn base_profile(name: &str) -> Profile {
    Profile {
        name: name.to_string(),
        version: "1.0.0".to_string(),
        description: "base fixture".to_string(),
        protected: false,
        created_at: String::new(),
        cloned_from: None,
        settings: ProfileSettings::default(),
        pipeline: vec![StateDef {
            state_id: "phase1".to_string(),
            name: "Phase 1".to_string(),
            description: String::new(),
            system_prompt: None,
            enabled: true,
            max_iterations: 3,
            loop_state: false,
            icon: None,
            sub_states: vec![
                SubStateDef {
                    id: "inject".to_string(),
                    sub_state_type: SubStateType::Inject,
                    name: "Inject".to_string(),
                    description: String::new(),
                    enabled: true,
                    criteria: None,
                    inject: None,
                    verify: None,
                    approver_pubkey: None,
                    approval_prompt: None,
                    hooks: Vec::new(),
                },
                SubStateDef {
                    id: "work".to_string(),
                    sub_state_type: SubStateType::Execute,
                    name: "Work".to_string(),
                    description: String::new(),
                    enabled: true,
                    criteria: None,
                    inject: None,
                    verify: None,
                    approver_pubkey: None,
                    approval_prompt: None,
                    hooks: Vec::new(),
                },
                SubStateDef {
                    id: "checklist".to_string(),
                    sub_state_type: SubStateType::Checklist,
                    name: "Checklist".to_string(),
                    description: String::new(),
                    enabled: true,
                    criteria: Some(vec!["done".to_string()]),
                    inject: None,
                    verify: None,
                    approver_pubkey: None,
                    approval_prompt: None,
                    hooks: Vec::new(),
                },
            ],
            hooks: Vec::new(),
        }],
        output_contract: None,
    }
}

/// Fresh temp profiles dir + `ProfileEditor`, seeded with `base_profile`.
fn setup() -> (TempDir, ProfileEditor) {
    let dir = TempDir::new().expect("create temp profiles dir");
    let manager = ProfileManager::new(dir.path());
    manager
        .save_profile(&base_profile("base"))
        .expect("seed base profile");
    let editor = ProfileEditor::with_manager(ProfileManager::new(dir.path()), fixture_library());
    (dir, editor)
}

// ---------------------------------------------------------------------
// 5. add/remove/move sub + macro
// ---------------------------------------------------------------------

#[test]
fn add_macro_scaffold_is_immediately_valid() {
    let (_dir, editor) = setup();
    let profile = editor
        .add_macro("base", "phase2", "Phase 2", None)
        .expect("scaffolded macro must be valid immediately");
    assert_eq!(profile.pipeline.len(), 2);
    assert_eq!(profile.pipeline[1].state_id, "phase2");
    let subs = &profile.pipeline[1].sub_states;
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].sub_state_type, SubStateType::Checklist);
}

#[test]
fn add_sub_checklist_scaffold_has_non_empty_criteria_but_a_2nd_checklist_is_rejected() {
    let (_dir, editor) = setup();
    // A macro may have only ONE trailing checklist (Rule 3) -- adding a 2nd
    // checklist sub-state must be rejected, proving the checklist scaffold
    // itself (which the accepted `add_macro` path already exercises) is
    // subject to the same strict validate-on-write as everything else.
    let err = editor
        .add_sub(
            "base",
            "phase1",
            "checklist_2",
            SubStateType::Checklist,
            "Second Checklist",
            None,
        )
        .expect_err("a second checklist sub-state must be rejected");
    assert!(matches!(err, EditError::Validation(_)));
}

#[test]
fn set_criteria_on_a_checklist_sub_updates_it() {
    let (_dir, editor) = setup();
    let profile = editor
        .set_criteria(
            "base",
            "phase1",
            "checklist",
            vec!["a".to_string(), "b".to_string()],
        )
        .expect("set_criteria on existing checklist should succeed");
    let sub = &profile.pipeline[0].sub_states[2];
    assert_eq!(sub.criteria, Some(vec!["a".to_string(), "b".to_string()]));
}

#[test]
fn add_sub_non_checklist_defaults_before_trailing_checklist() {
    let (_dir, editor) = setup();
    let profile = editor
        .add_sub(
            "base",
            "phase1",
            "review",
            SubStateType::Review,
            "Review",
            None,
        )
        .expect("scaffold must stay valid (inserted before the checklist)");
    let ids: Vec<&str> = profile.pipeline[0]
        .sub_states
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(ids, vec!["inject", "work", "review", "checklist"]);
}

#[test]
fn remove_sub_keeps_validity_or_is_rejected() {
    let (_dir, editor) = setup();
    // Removing the non-checklist "work" sub keeps the macro valid.
    let profile = editor
        .remove_sub("base", "phase1", "work")
        .expect("removing a non-essential sub should succeed");
    assert_eq!(profile.pipeline[0].sub_states.len(), 2);

    // Removing the last remaining checklist is rejected (Rule 3).
    let err = editor
        .remove_sub("base", "phase1", "checklist")
        .expect_err("removing the only checklist is invalid");
    assert!(matches!(err, EditError::Validation(_)));
}

#[test]
fn move_sub_reorders() {
    let (_dir, editor) = setup();
    let profile = editor
        .move_sub("base", "phase1", "work", 0)
        .expect("move should keep the profile valid");
    let ids: Vec<&str> = profile.pipeline[0]
        .sub_states
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(ids, vec!["work", "inject", "checklist"]);
}

#[test]
fn remove_macro_and_move_macro() {
    let (_dir, editor) = setup();
    editor.add_macro("base", "phase2", "Phase 2", None).unwrap();
    editor.add_macro("base", "phase3", "Phase 3", None).unwrap();

    let profile = editor
        .move_macro("base", "phase3", 0)
        .expect("move_macro should succeed");
    assert_eq!(profile.pipeline[0].state_id, "phase3");

    let profile = editor
        .remove_macro("base", "phase2")
        .expect("remove_macro should succeed");
    let ids: Vec<&str> = profile
        .pipeline
        .iter()
        .map(|m| m.state_id.as_str())
        .collect();
    assert_eq!(ids, vec!["phase3", "phase1"]);
}

#[test]
fn set_macro_fields() {
    let (_dir, editor) = setup();
    let profile = editor
        .set_macro_name("base", "phase1", "Renamed")
        .expect("rename macro");
    assert_eq!(profile.pipeline[0].name, "Renamed");

    let profile = editor
        .set_macro_description("base", "phase1", "new desc")
        .expect("set macro description");
    assert_eq!(profile.pipeline[0].description, "new desc");

    let profile = editor
        .set_macro_max_iterations("base", "phase1", 7)
        .expect("set max iterations");
    assert_eq!(profile.pipeline[0].max_iterations, 7);

    let err = editor
        .disable_macro("base", "phase1")
        .expect_err("disabling the only macro leaves an empty normalized pipeline");
    assert!(
        matches!(err, EditError::Validation(_)),
        "must fail validation (empty pipeline), not some unrelated error: {err:?}"
    );
}

#[test]
fn set_macro_loop_rejected_on_final_macro() {
    let (_dir, editor) = setup();
    let err = editor
        .set_macro_loop("base", "phase1", true)
        .expect_err("loop:true on the final macro is forbidden");
    assert!(matches!(err, EditError::Validation(_)));
}

// ---------------------------------------------------------------------
// 6. name_collision
// ---------------------------------------------------------------------

#[test]
fn create_onto_existing_name_is_already_exists_unless_forced() {
    let (_dir, editor) = setup();
    let err = editor
        .create("base", "dup", false)
        .expect_err("create over existing unprotected name must fail without force");
    assert!(matches!(err, EditError::AlreadyExists(_)));

    editor
        .create("base", "forced over", true)
        .expect("force=true should allow overwrite");
}

#[test]
fn clone_onto_existing_name_is_already_exists_unless_forced() {
    let (_dir, editor) = setup();
    editor.create("other", "d", false).unwrap();

    let err = editor
        .clone("base", "other", false)
        .expect_err("clone over existing unprotected name must fail without force");
    assert!(matches!(err, EditError::AlreadyExists(_)));

    editor
        .clone("base", "other", true)
        .expect("force=true should allow overwrite");
}

#[test]
fn create_and_clone_never_clobber_a_protected_profile_even_with_force() {
    let dir = TempDir::new().unwrap();
    let mut protected = base_profile("locked");
    protected.protected = true;
    // `save_profile` refuses to write ANY in-memory `protected: true`
    // profile (by design -- protected profiles are shipped statically, not
    // authored through the API), so write this fixture straight to disk.
    std::fs::write(
        dir.path().join("locked.yaml"),
        serde_yaml::to_string(&protected).unwrap(),
    )
    .unwrap();

    let editor = ProfileEditor::with_manager(ProfileManager::new(dir.path()), fixture_library());
    let err = editor
        .create("locked", "x", true)
        .expect_err("protected target must never be overwritten, even with force");
    assert!(matches!(err, EditError::Protected(_)));
}

// ---------------------------------------------------------------------
// rename + conflict-adjacent
// ---------------------------------------------------------------------

#[test]
fn rename_conflict_when_target_already_exists() {
    let (_dir, editor) = setup();
    editor.create("taken", "d", false).unwrap();
    let err = editor
        .rename("base", "taken", false)
        .expect_err("rename onto existing unprotected name must fail without force");
    assert!(matches!(err, EditError::AlreadyExists(_)));
}

#[test]
fn rename_moves_the_file_and_updates_the_name_field() {
    let (dir, editor) = setup();
    let profile = editor
        .rename("base", "renamed", false)
        .expect("rename should succeed");
    assert_eq!(profile.name, "renamed");
    assert!(!dir.path().join("base.yaml").exists());
    assert!(dir.path().join("renamed.yaml").exists());
}

// ---------------------------------------------------------------------
// Protected / not-found / unsafe-name guards + remaining profile verbs
// ---------------------------------------------------------------------

#[test]
fn mutations_on_protected_profile_are_rejected() {
    let dir = TempDir::new().unwrap();
    let mut protected = base_profile("locked");
    protected.protected = true;
    std::fs::write(
        dir.path().join("locked.yaml"),
        serde_yaml::to_string(&protected).unwrap(),
    )
    .unwrap();

    let editor = ProfileEditor::with_manager(ProfileManager::new(dir.path()), fixture_library());
    let err = editor
        .set_description("locked", "nope")
        .expect_err("protected profile must reject mutation");
    assert!(matches!(err, EditError::Protected(_)));
}

#[test]
fn not_found_and_unsafe_name_are_reported() {
    let (_dir, editor) = setup();
    let err = editor.set_description("does-not-exist", "x").unwrap_err();
    assert!(matches!(err, EditError::NotFound(_)));

    let err = editor.set_description("../escape", "x").unwrap_err();
    assert!(matches!(err, EditError::UnsafeName(_)));
}

#[test]
fn delete_removes_an_unprotected_profile() {
    let (dir, editor) = setup();
    editor.delete("base").expect("delete should succeed");
    assert!(!dir.path().join("base.yaml").exists());
}

#[test]
fn set_settings_updates_only_provided_fields() {
    let (_dir, editor) = setup();
    let profile = editor
        .set_settings("base", Some(5), None, None)
        .expect("set_settings should succeed");
    assert_eq!(profile.settings.oscillation_detection_threshold, 5);
    assert!(profile.settings.auto_loop_on_checklist_failure); // default true, unchanged
    assert_eq!(profile.settings.global_timeout_seconds, 3600); // default, unchanged

    let profile = editor
        .set_settings("base", None, Some(false), None)
        .expect("set_settings should succeed");
    assert_eq!(profile.settings.oscillation_detection_threshold, 5);
    assert!(!profile.settings.auto_loop_on_checklist_failure);

    // global_timeout is independently settable, including the 0=disabled sentinel.
    let profile = editor
        .set_settings("base", None, None, Some(0))
        .expect("set_settings should succeed");
    assert_eq!(profile.settings.global_timeout_seconds, 0);
    assert_eq!(profile.settings.oscillation_detection_threshold, 5); // untouched
}

#[test]
fn set_verify_targeting_undeclared_criterion_is_rejected() {
    let (_dir, editor) = setup();
    let err = editor
        .set_verify(
            "base",
            "phase1",
            "checklist",
            vec![protocol_types::profile::VerifyCheck {
                criterion: "not_declared".to_string(),
                command: "true".to_string(),
                expect_exit: 0,
            }],
        )
        .expect_err("verify targeting an undeclared criterion is invalid");
    assert!(matches!(err, EditError::Validation(_)));
}
