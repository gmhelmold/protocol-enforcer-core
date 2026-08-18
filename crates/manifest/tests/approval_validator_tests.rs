//! `approver_pubkey` required iff `human_approval`, in the same per-rule
//! style as `profile_validator_tests.rs`.
//!
//! This is the authoring-time half of the gate: a `human_approval`
//! sub-state whose key is absent or unparseable has NOTHING to verify a
//! signature against, so the profile must be refused rather than loaded into a
//! gate that cannot gate. The reverse direction matters just as much — a key on
//! an `execute` sub-state is an author who thinks they built a gate and did not.

use protocol_library::Library;
use protocol_manifest::{load_profile, validate_profile};
use std::io::Write;
use std::path::PathBuf;

/// A real Ed25519 public key (the demo key from `profiles/human-gate-demo.yaml`,
/// whose seed is published and worthless).
const DEMO_PUBKEY: &str = "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c";

fn fixture_library() -> Library {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library");
    Library::new(root)
}

fn load(yaml: &str) -> protocol_types::Profile {
    let mut f = tempfile::NamedTempFile::new().expect("tmp file");
    write!(f, "{yaml}").expect("write tmp file");
    load_profile(f.path()).expect("profile should parse")
}

/// One macro with a `human_approval` sub-state carrying `key_line`, then the
/// mandatory trailing checklist.
fn approval_profile(sub_type: &str, key_line: &str) -> String {
    format!(
        r#"
name: "approval-fixture"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: approve
    name: "Approve"
    sub_states:
      - id: human_gate
        type: {sub_type}
        name: "Human Sign-Off"
{key_line}
      - id: check
        type: checklist
        name: "Check"
        criteria:
          - "approved"
"#
    )
}

#[test]
fn human_approval_with_a_valid_pubkey_passes() {
    let yaml = approval_profile(
        "human_approval",
        &format!("        approver_pubkey: \"{DEMO_PUBKEY}\""),
    );
    let profile = load(&yaml);
    assert!(validate_profile(&profile, &fixture_library()).is_ok());
}

#[test]
fn approval_prompt_is_optional_and_does_not_affect_validity() {
    let key_lines = format!(
        "        approver_pubkey: \"{DEMO_PUBKEY}\"\n        approval_prompt: \"Please review and sign.\""
    );
    let yaml = approval_profile("human_approval", &key_lines);
    let profile = load(&yaml);
    assert!(validate_profile(&profile, &fixture_library()).is_ok());
    let sub = &profile.pipeline[0].sub_states[0];
    assert_eq!(
        sub.approval_prompt.as_deref(),
        Some("Please review and sign.")
    );
}

#[test]
fn human_approval_without_a_pubkey_fails() {
    let profile = load(&approval_profile("human_approval", ""));
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("requires approver_pubkey") && e.contains("human_gate")),
        "{errs:?}"
    );
}

#[test]
fn human_approval_with_malformed_hex_fails() {
    for bad in [
        "not-hex-at-all",
        "deadbeef",                            // right alphabet, wrong length
        &"zz".repeat(32),                      // right length, wrong alphabet
        &DEMO_PUBKEY[..DEMO_PUBKEY.len() - 1], // odd length
    ] {
        let yaml = approval_profile(
            "human_approval",
            &format!("        approver_pubkey: \"{bad}\""),
        );
        let profile = load(&yaml);
        let errs = match validate_profile(&profile, &fixture_library()) {
            Err(e) => e,
            Ok(()) => panic!("'{bad}' must be rejected"),
        };
        assert!(
            errs.iter()
                .any(|e| e.contains("not a valid 32-byte hex Ed25519 public key")),
            "{bad}: {errs:?}"
        );
    }
}

#[test]
fn a_pubkey_on_a_non_approval_sub_state_fails() {
    let yaml = approval_profile(
        "execute",
        &format!("        approver_pubkey: \"{DEMO_PUBKEY}\""),
    );
    let profile = load(&yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("only allowed on a human_approval sub-state")),
        "{errs:?}"
    );
}

#[test]
fn the_shipped_demo_profile_validates() {
    // Non-vacuity for everything above: the profile we actually ship, loaded
    // from disk, must pass the very rule these tests exercise.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../profiles/human-gate-demo.yaml")
        .canonicalize()
        .expect("shipped demo profile");
    let profile = load_profile(&path).expect("demo profile loads");
    let lib = Library::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../library")
            .canonicalize()
            .expect("library dir"),
    );
    assert!(validate_profile(&profile, &lib).is_ok());
}
