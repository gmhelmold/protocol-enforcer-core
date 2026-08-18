//! WP0 round-trip test: every checked-in profile YAML must deserialize into
//! the promoted `protocol_types::Profile` (SPEC_v3 §1/§1.1) with a
//! non-empty pipeline.

use protocol_types::Profile;
use std::path::PathBuf;

fn profiles_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("profiles")
}

fn load(name: &str) -> Profile {
    let path = profiles_dir().join(format!("{name}.yaml"));
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {path:?}: {e}"));
    serde_yaml::from_str(&content).unwrap_or_else(|e| {
        panic!("failed to deserialize {path:?} as protocol_types::Profile: {e}")
    })
}

#[test]
fn default_profile_round_trips() {
    let profile = load("default");
    assert!(
        !profile.pipeline.is_empty(),
        "default.yaml pipeline must be non-empty"
    );
}

#[test]
fn hacked_profile_round_trips() {
    let profile = load("hacked");
    assert!(
        !profile.pipeline.is_empty(),
        "hacked.yaml pipeline must be non-empty"
    );
}

#[test]
fn research_profile_round_trips() {
    let profile = load("research");
    assert!(
        !profile.pipeline.is_empty(),
        "research.yaml pipeline must be non-empty"
    );
}

#[test]
fn quick_bug_fix_profile_round_trips() {
    let profile = load("quick-bug-fix");
    assert!(
        !profile.pipeline.is_empty(),
        "quick-bug-fix.yaml pipeline must be non-empty"
    );
}
