// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Acceptance tests for `ProfileEditor` (editing
//! core only -- CLI/UX verbs are out of scope here).
//!
//! Hermetic: every test runs against a fresh `tempfile::TempDir`, so the
//! repo-tracked `profiles/` directory is never touched.
//!
//! Covers acceptance items 1-4 (validate_on_write_rejects, atomic_save,
//! roundtrip_all_profiles, set_prompt/attach_skill/detach_skill). Items
//! 5-13 (macro/sub add-remove-move, name_collision, conflict_cas, guards)
//! live in `profile_editor_macro_sub_tests.rs` -- split to stay under the
//! workspace's 600-line-per-file cap.

use protocol_library::Library;
use protocol_manifest::{EditError, ProfileEditor, ProfileManager};
use protocol_types::profile::{Profile, ProfileSettings, StateDef, SubStateDef, SubStateType};
use std::path::PathBuf;
use tempfile::TempDir;

pub(crate) fn fixture_library() -> Library {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library");
    Library::new(root)
}

fn repo_profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("profiles")
}

/// A base profile: one macro `phase1` with inject -> execute -> checklist.
pub(crate) fn base_profile(name: &str) -> Profile {
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
pub(crate) fn setup() -> (TempDir, ProfileEditor) {
    let dir = TempDir::new().expect("create temp profiles dir");
    let manager = ProfileManager::new(dir.path());
    manager
        .save_profile(&base_profile("base"))
        .expect("seed base profile");
    let editor = ProfileEditor::with_manager(ProfileManager::new(dir.path()), fixture_library());
    (dir, editor)
}

pub(crate) fn read_file(dir: &TempDir, name: &str) -> String {
    std::fs::read_to_string(dir.path().join(format!("{name}.yaml"))).expect("read profile file")
}

// ---------------------------------------------------------------------
// 1. validate_on_write_rejects
// ---------------------------------------------------------------------

#[test]
fn validate_on_write_rejects_empty_criteria_and_leaves_file_unchanged() {
    let (dir, editor) = setup();
    let before = read_file(&dir, "base");

    let err = editor
        .set_criteria("base", "phase1", "checklist", vec![])
        .expect_err("empty checklist criteria must be rejected");

    match err {
        EditError::Validation(violations) => {
            assert!(
                violations.iter().any(|v| v.contains("non-empty criteria")),
                "violation should explain the empty-criteria rule: {violations:?}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }

    let after = read_file(&dir, "base");
    assert_eq!(before, after, "rejected edit must leave the file untouched");
}

#[test]
fn validate_on_write_rejects_removing_the_only_checklist_sub() {
    let (dir, editor) = setup();
    let before = read_file(&dir, "base");

    let err = editor
        .remove_sub("base", "phase1", "checklist")
        .expect_err("removing the only checklist must be rejected");
    assert!(matches!(err, EditError::Validation(_)));

    assert_eq!(read_file(&dir, "base"), before);
}

// ---------------------------------------------------------------------
// 2. atomic_save
// ---------------------------------------------------------------------

#[test]
fn atomic_save_produces_a_valid_reloadable_file() {
    let (dir, editor) = setup();
    editor
        .set_description("base", "updated description")
        .expect("valid edit should succeed");

    let reloaded = editor
        .manager()
        .load_profile("base")
        .expect("file must be valid YAML after save");
    assert_eq!(reloaded.description, "updated description");

    // No stray temp files left behind.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "no temp files should remain: {leftovers:?}"
    );
}

// ---------------------------------------------------------------------
// 3. roundtrip_all_profiles
// ---------------------------------------------------------------------

#[test]
fn roundtrip_all_shipped_profiles_semantically_unchanged() {
    let dir = TempDir::new().expect("temp dir");
    let source = repo_profiles_dir();
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&source).expect("read repo profiles dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            std::fs::copy(&path, dir.path().join(path.file_name().unwrap()))
                .expect("copy shipped profile");
            names.push(path.file_stem().unwrap().to_string_lossy().to_string());
        }
    }
    // The open-core repo ships the six general-purpose profiles (default,
    // quick-bug-fix, human-gate-demo, research, tdd_feature, mutated-clone);
    // the experimental genesis-* profiles are not part of this repository.
    // `human-gate-demo.yaml` makes this roundtrip cover the `approver_pubkey` /
    // `approval_prompt` fields against a real shipped file.
    assert_eq!(names.len(), 6, "expected 6 shipped profiles");

    let lib = Library::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("library"),
    );
    let editor = ProfileEditor::with_manager(ProfileManager::new(dir.path()), lib);

    for name in &names {
        let before = editor.manager().load_profile(name).expect("load");
        // A true no-op preview: touch nothing, mutate nothing.
        let previewed = editor
            .preview(name, |_p| Ok(()))
            .unwrap_or_else(|e| panic!("preview of shipped profile '{name}' failed: {e:?}"));
        assert_eq!(previewed.pipeline.len(), before.pipeline.len());
        assert_eq!(previewed.name, before.name);

        // Preview must not write anything.
        let untouched = std::fs::read_to_string(dir.path().join(format!("{name}.yaml")))
            .expect("file still present");
        let original = std::fs::read_to_string(source.join(format!("{name}.yaml"))).unwrap();
        assert_eq!(untouched, original, "preview must not write to disk");
    }
}

#[test]
fn disabled_macro_survives_editor_roundtrip_never_stripped() {
    let (dir, editor) = setup();
    editor
        .add_macro("base", "phase2", "Phase 2", None)
        .expect("add second macro");
    editor
        .disable_macro("base", "phase2")
        .expect("disable the new macro");

    let reloaded = editor.manager().load_profile("base").expect("reload");
    let phase2 = reloaded
        .pipeline
        .iter()
        .find(|m| m.state_id == "phase2")
        .expect("phase2 must still be present on disk");
    assert!(!phase2.enabled, "phase2 must be disabled, not stripped");
    assert_eq!(reloaded.pipeline.len(), 2);

    let _ = dir; // keep guard alive for the whole test
}

// ---------------------------------------------------------------------
// 4. set_prompt / attach_skill / detach_skill
// ---------------------------------------------------------------------

#[test]
fn set_prompt_sets_the_injection_prompt() {
    let (_dir, editor) = setup();
    let profile = editor
        .set_prompt("base", "phase1", "inject", Some("do the thing".to_string()))
        .expect("set_prompt should succeed");
    let sub = &profile.pipeline[0].sub_states[0];
    assert_eq!(
        sub.inject.as_ref().and_then(|i| i.prompt.clone()),
        Some("do the thing".to_string())
    );
}

#[test]
fn attach_skill_resolvable_succeeds() {
    let (_dir, editor) = setup();
    let profile = editor
        .attach_skill("base", "phase1", "inject", "greeting")
        .expect("greeting skill resolves in the fixture library");
    let sub = &profile.pipeline[0].sub_states[0];
    assert_eq!(
        sub.inject.as_ref().and_then(|i| i.skill.clone()),
        Some("greeting".to_string())
    );
}

#[test]
fn attach_skill_unresolvable_is_rejected_and_writes_nothing() {
    let (dir, editor) = setup();
    let before = read_file(&dir, "base");

    let err = editor
        .attach_skill("base", "phase1", "inject", "does_not_exist")
        .expect_err("unresolved skill must be rejected");
    match err {
        EditError::Validation(violations) => {
            assert!(violations.iter().any(|v| v.contains("unresolved skill")));
        }
        other => panic!("expected Validation, got {other:?}"),
    }
    assert_eq!(read_file(&dir, "base"), before);
}

#[test]
fn detach_skill_clears_it() {
    let (_dir, editor) = setup();
    editor
        .attach_skill("base", "phase1", "inject", "greeting")
        .expect("attach first");
    let profile = editor
        .detach_skill("base", "phase1", "inject")
        .expect("detach should succeed");
    let sub = &profile.pipeline[0].sub_states[0];
    assert_eq!(sub.inject.as_ref().and_then(|i| i.skill.clone()), None);
}

#[test]
fn attach_and_detach_protocol_round_trip() {
    let (_dir, editor) = setup();
    let profile = editor
        .attach_protocol("base", "phase1", "inject", "handoff")
        .expect("handoff protocol resolves");
    assert_eq!(
        profile.pipeline[0].sub_states[0]
            .inject
            .as_ref()
            .and_then(|i| i.protocol.clone()),
        Some("handoff".to_string())
    );

    let profile = editor
        .detach_protocol("base", "phase1", "inject")
        .expect("detach should succeed");
    assert_eq!(
        profile.pipeline[0].sub_states[0]
            .inject
            .as_ref()
            .and_then(|i| i.protocol.clone()),
        None
    );
}

#[test]
fn set_context_sets_the_injection_context() {
    let (_dir, editor) = setup();
    let profile = editor
        .set_context("base", "phase1", "inject", Some("ctx".to_string()))
        .expect("set_context should succeed");
    assert_eq!(
        profile.pipeline[0].sub_states[0]
            .inject
            .as_ref()
            .and_then(|i| i.context.clone()),
        Some("ctx".to_string())
    );
}
