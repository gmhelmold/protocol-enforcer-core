// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Macro-level `enabled`, normalized at load:
//! validator acceptance tests: at-least-one-enabled-macro, and the CRITICAL
//! case where disabling the true last macro promotes a `loop: true` middle
//! macro to final. Split out of `profile_validator_loop_tests.rs` to keep
//! that file under the 600-line cap (`scripts/check_loc.sh`).

use protocol_library::Library;
use protocol_manifest::{load_profile, validate_profile};
use std::io::Write;
use std::path::PathBuf;

fn fixture_library() -> Library {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library");
    Library::new(root)
}

fn load(yaml: &str) -> protocol_types::Profile {
    let mut f = tempfile::NamedTempFile::new().expect("tmp file");
    write!(f, "{yaml}").expect("write tmp file");
    load_profile(f.path()).expect("profile should parse")
}

#[test]
fn all_macros_disabled_is_a_validation_error() {
    let yaml = r#"
name: "all-disabled"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    enabled: false
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
  - state_id: beta
    name: "Beta"
    enabled: false
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["shipped"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("at least one macro must be enabled")),
        "expected the empty-normalized-pipeline error, got: {errs:?}"
    );
}

/// CRITICAL case: disabling the true last macro (`gamma`)
/// promotes the middle `loop: true` macro (`beta`) to the normalized final
/// macro. Rule 10 (keyed off the NORMALIZED pipeline) must catch this even
/// though `beta` is not the last macro in the raw/on-disk pipeline.
#[test]
fn disabling_true_last_macro_promotes_loop_middle_macro_to_final_and_is_rejected() {
    let yaml = r#"
name: "disabled-last-promotes-loop-macro"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
  - state_id: beta
    name: "Beta"
    loop: true
    sub_states:
      - id: work
        type: execute
        name: "Work"
      - id: check
        type: checklist
        name: "Check"
        criteria: ["shipped"]
  - state_id: gamma
    name: "Gamma"
    enabled: false
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["delivered"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("macro 'beta'")
                && e.contains("forbidden on the final pipeline macro")),
        "expected Rule 10 to fire on 'beta' once 'gamma' is disabled, got: {errs:?}"
    );
}

/// Positive counterpart: the same shape, but `gamma` stays enabled -- `beta`
/// is genuinely non-final and this must validate clean.
#[test]
fn loop_middle_macro_is_fine_when_true_last_macro_stays_enabled() {
    let yaml = r#"
name: "disabled-last-not-triggered"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
  - state_id: beta
    name: "Beta"
    loop: true
    sub_states:
      - id: work
        type: execute
        name: "Work"
      - id: check
        type: checklist
        name: "Check"
        criteria: ["shipped"]
  - state_id: gamma
    name: "Gamma"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["delivered"]
"#;
    let profile = load(yaml);
    assert!(validate_profile(&profile, &fixture_library()).is_ok());
}

/// A disabled macro must still be structurally well-formed (Rule 1/2/3/4/5/8
/// run on the FULL profile, not just the normalized one) -- e.g. a disabled
/// macro with an empty `sub_states` is still rejected.
#[test]
fn disabled_macro_must_still_be_structurally_well_formed() {
    let yaml = r#"
name: "disabled-still-structural"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
  - state_id: beta
    name: "Beta"
    enabled: false
    sub_states: []
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("macro 'beta'") && e.contains("sub_states must not be empty")),
        "expected the disabled macro to still be structurally validated, got: {errs:?}"
    );
}
