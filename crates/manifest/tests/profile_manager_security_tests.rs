//! Security-focused tests for `ProfileManager`:
//! - profile-name path traversal is rejected on every name-to-path
//!   operation (load/save/delete/clone/create).
//! - the protected-guard cannot be bypassed by cloning/creating a profile
//!   with `protected = false` in memory onto a target path that already
//!   holds a protected profile on disk.
//!
//! Hermetic: every test runs against a fresh `tempfile::TempDir`, seeded
//! only with the fixture profiles it needs, so the repo-tracked
//! `profiles/` directory is never touched.

use protocol_manifest::ProfileManager;
use protocol_types::profile::{Profile, ProfileSettings};
use tempfile::TempDir;

fn empty_profile(name: &str, protected: bool) -> Profile {
    Profile {
        name: name.to_string(),
        version: "1.0.0".to_string(),
        description: "test profile".to_string(),
        protected,
        created_at: chrono::Utc::now().to_rfc3339(),
        cloned_from: None,
        settings: ProfileSettings::default(),
        pipeline: vec![],
        output_contract: None,
    }
}

fn setup() -> (TempDir, ProfileManager) {
    let dir = TempDir::new().expect("create temp profiles dir");
    let manager = ProfileManager::new(dir.path());
    (dir, manager)
}

// ---------------------------------------------------------------------
// Path traversal rejection
// ---------------------------------------------------------------------

#[test]
fn load_profile_rejects_traversal_name() {
    let (_dir, manager) = setup();
    let result = manager.load_profile("../../etc/passwd");
    assert!(result.is_err(), "traversal name must be rejected");
}

#[test]
fn load_profile_rejects_slash_name() {
    let (_dir, manager) = setup();
    let result = manager.load_profile("sub/dir");
    assert!(result.is_err(), "name containing '/' must be rejected");
}

#[test]
fn save_profile_rejects_traversal_name() {
    let (_dir, manager) = setup();
    let profile = empty_profile("../escape", false);
    let result = manager.save_profile(&profile);
    assert!(result.is_err(), "traversal name must be rejected on save");
}

#[test]
fn delete_profile_rejects_traversal_name() {
    let (_dir, manager) = setup();
    let result = manager.delete_profile("../../etc/passwd");
    assert!(result.is_err(), "traversal name must be rejected on delete");
}

#[test]
fn clone_profile_rejects_traversal_source_name() {
    let (_dir, manager) = setup();
    let result = manager.clone_profile("../../etc/passwd", "safe-name");
    assert!(
        result.is_err(),
        "traversal source name must be rejected on clone"
    );
}

#[test]
fn clone_profile_rejects_traversal_new_name() {
    let (dir, manager) = setup();
    let source = empty_profile("source", false);
    std::fs::write(
        dir.path().join("source.yaml"),
        serde_yaml::to_string(&source).unwrap(),
    )
    .unwrap();

    let result = manager.clone_profile("source", "../../etc/passwd");
    assert!(
        result.is_err(),
        "traversal new name must be rejected on clone"
    );
}

#[test]
fn create_empty_profile_rejects_traversal_name() {
    let (_dir, manager) = setup();
    let result = manager.create_empty_profile("../../etc/passwd", "desc");
    assert!(result.is_err(), "traversal name must be rejected on create");
}

// ---------------------------------------------------------------------
// Protected-guard bypass
// ---------------------------------------------------------------------

#[test]
fn clone_into_existing_protected_target_is_refused() {
    let (dir, manager) = setup();

    // Seed a protected "default" profile directly on disk.
    let protected = empty_profile("default", true);
    std::fs::write(
        dir.path().join("default.yaml"),
        serde_yaml::to_string(&protected).unwrap(),
    )
    .unwrap();

    // Seed a non-protected "source" profile to clone from.
    let source = empty_profile("source", false);
    std::fs::write(
        dir.path().join("source.yaml"),
        serde_yaml::to_string(&source).unwrap(),
    )
    .unwrap();

    // clone_profile forces protected=false on the in-memory clone, but the
    // on-disk "default.yaml" is protected, so the write must be refused.
    let result = manager.clone_profile("source", "default");
    assert!(
        result.is_err(),
        "clone must not be able to clobber a protected on-disk profile"
    );

    // The protected file on disk must be untouched.
    let still_protected = manager.load_profile("default").unwrap();
    assert!(still_protected.protected);
    assert_eq!(still_protected.name, "default");
}

#[test]
fn create_empty_profile_into_existing_protected_target_is_refused() {
    let (dir, manager) = setup();

    let protected = empty_profile("default", true);
    std::fs::write(
        dir.path().join("default.yaml"),
        serde_yaml::to_string(&protected).unwrap(),
    )
    .unwrap();

    let result = manager.create_empty_profile("default", "attempt to overwrite");
    assert!(
        result.is_err(),
        "create_empty_profile must not clobber a protected on-disk profile"
    );
}

#[test]
fn clone_to_fresh_name_still_works() {
    let (dir, manager) = setup();

    let source = empty_profile("source", false);
    std::fs::write(
        dir.path().join("source.yaml"),
        serde_yaml::to_string(&source).unwrap(),
    )
    .unwrap();

    let cloned = manager
        .clone_profile("source", "brand-new-name")
        .expect("cloning to a fresh name must succeed");
    assert_eq!(cloned.name, "brand-new-name");
    assert!(!cloned.protected);
    assert_eq!(cloned.cloned_from, Some("source".to_string()));

    let reloaded = manager.load_profile("brand-new-name").unwrap();
    assert_eq!(reloaded.name, "brand-new-name");
}

#[test]
fn clone_to_existing_non_protected_target_still_works() {
    let (dir, manager) = setup();

    let source = empty_profile("source", false);
    std::fs::write(
        dir.path().join("source.yaml"),
        serde_yaml::to_string(&source).unwrap(),
    )
    .unwrap();

    let existing_target = empty_profile("target", false);
    std::fs::write(
        dir.path().join("target.yaml"),
        serde_yaml::to_string(&existing_target).unwrap(),
    )
    .unwrap();

    let result = manager.clone_profile("source", "target");
    assert!(
        result.is_ok(),
        "cloning onto an existing non-protected target must still succeed"
    );
}
