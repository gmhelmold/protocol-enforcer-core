//! SPEC_loopback.md validator acceptance tests (Rules 9/10 -- #7
//! `validate_loop_without_execute`, #7b `validate_loop_on_final_macro`, #13
//! `validate_loop_execute_disabled` -- plus the profile-compat check), split
//! out of `profile_validator_tests.rs` to keep that file under the 600-line
//! cap (`scripts/check_loc.sh`).

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

// ---------------------------------------------------------------------
// Rules 9/10 (SPEC_loopback.md): loop_state structural validation.
// ---------------------------------------------------------------------

#[test]
fn validate_loop_without_execute() {
    // `alpha` (non-final macro) has loop: true but no Execute sub-state at
    // all -- nothing to loop back to. `beta` is the final macro, plain.
    let yaml = r#"
name: "loop-without-execute"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    loop: true
    sub_states:
      - id: intro
        type: inject
        name: "Intro"
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
  - state_id: beta
    name: "Beta"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["shipped"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter().any(|e| e.contains("macro 'alpha'")
            && e.contains("loop: true")
            && e.contains("found none")),
        "expected a Rule 9 violation for macro 'alpha', got: {errs:?}"
    );
}

#[test]
fn validate_loop_on_final_macro() {
    // `beta` is the LAST pipeline macro, has loop: true, AND has an enabled
    // execute sub (so Rule 9 does not also fire) -- Rule 10 alone must
    // reject this: the delivery step must not cycle.
    let yaml = r#"
name: "loop-on-final-macro"
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
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("macro 'beta'")
                && e.contains("forbidden on the final pipeline macro")),
        "expected a Rule 10 violation for macro 'beta', got: {errs:?}"
    );
}

#[test]
fn validate_loop_execute_disabled() {
    // `alpha`'s only Execute sub-state exists but is `enabled: false` --
    // distinct fixture from "no execute sub at all", same Rule 9 predicate
    // (`first_enabled_execute_index` -- shared with the engine's loop-back
    // target resolution, so the two can't drift).
    let yaml = r#"
name: "loop-execute-disabled"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    loop: true
    sub_states:
      - id: work
        type: execute
        name: "Work"
        enabled: false
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
  - state_id: beta
    name: "Beta"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["shipped"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter().any(|e| e.contains("macro 'alpha'")
            && e.contains("loop: true")
            && e.contains("found none")),
        "expected a Rule 9 violation (disabled-only execute sub), got: {errs:?}"
    );
}

#[test]
fn validate_loop_with_enabled_execute_on_non_final_macro_passes() {
    // Regression/positive: a non-final macro with loop: true and an enabled
    // execute sub-state validates clean (mirrors default.yaml's shape).
    let yaml = r#"
name: "loop-ok"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    loop: true
    sub_states:
      - id: work
        type: execute
        name: "Work"
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
  - state_id: beta
    name: "Beta"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["shipped"]
"#;
    let profile = load(yaml);
    assert!(validate_profile(&profile, &fixture_library()).is_ok());
}

/// SPEC_loopback.md profile-compat check: all 5 shipped `profiles/*.yaml`
/// must still validate clean under the new Rules 9/10 (`default.yaml`'s
/// `loop: true` execute macro has execute subs and isn't final;
/// `hacked.yaml`/`quick-bug-fix.yaml` mirror that same shape).
#[test]
fn all_shipped_profiles_validate_clean_under_loop_rules() {
    let names = [
        "default.yaml",
        "hacked.yaml",
        "quick-bug-fix.yaml",
        "research.yaml",
        "tdd_feature.yaml",
    ];
    // The shipped profiles' `inject.skill`/`inject.protocol` refs resolve
    // against the real workspace `./library`, not this crate's own test
    // fixture library -- `cargo test`'s cwd is the package root, so this
    // must be an absolute path, not `Library::from_env()`'s relative
    // default.
    let workspace_library =
        Library::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../library"));
    for name in names {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../profiles/{name}"));
        let profile = load_profile(&path).unwrap_or_else(|e| panic!("{name} should parse: {e}"));
        let result = validate_profile(&profile, &workspace_library);
        assert!(
            result.is_ok(),
            "{name} should validate clean under Rules 9/10: {result:?}"
        );
    }
}
