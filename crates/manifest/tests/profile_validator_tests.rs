//! Per-rule tests for the strict nested-profile validator (SPEC_v3 §4, WP1).
//!
//! Each rule gets a passing case plus at least one failing fixture, per the
//! WP1 DoD. `load_profile`/`validate_profile` are exercised together.

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

const BASE_VALID: &str = r#"
name: "base"
version: "1.0.0"
description: "baseline valid profile"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: do_work
        type: execute
        name: "Do work"
      - id: check
        type: checklist
        name: "Check"
        criteria:
          - "done"
"#;

// ---------------------------------------------------------------------
// Rule 1: pipeline non-empty; each macro sub_states non-empty.
// ---------------------------------------------------------------------

#[test]
fn rule1_valid_pipeline_passes() {
    let profile = load(BASE_VALID);
    assert!(validate_profile(&profile, &fixture_library()).is_ok());
}

#[test]
fn rule1_empty_pipeline_fails() {
    let yaml = r#"
name: "empty-pipeline"
version: "1.0.0"
description: "d"
pipeline: []
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("pipeline must not be empty")),
        "{errs:?}"
    );
}

#[test]
fn rule1_empty_sub_states_fails() {
    let yaml = r#"
name: "empty-subs"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states: []
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("sub_states must not be empty")),
        "{errs:?}"
    );
}

// ---------------------------------------------------------------------
// Rule 2: unique macro state_id; unique sub id within a macro.
// ---------------------------------------------------------------------

#[test]
fn rule2_duplicate_macro_state_id_fails() {
    let yaml = r#"
name: "dup-macro"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha 1"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
  - state_id: alpha
    name: "Alpha 2"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("duplicate macro state_id: alpha")),
        "{errs:?}"
    );
}

#[test]
fn rule2_duplicate_sub_id_fails() {
    let yaml = r#"
name: "dup-sub"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: same
        type: execute
        name: "Do work"
      - id: same
        type: checklist
        name: "Check"
        criteria: ["done"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("duplicate sub-state id: same")),
        "{errs:?}"
    );
}

// ---------------------------------------------------------------------
// Rule 3: exactly one Checklist sub-state per macro, last, not disabled.
// ---------------------------------------------------------------------

#[test]
fn rule3_two_checklists_fails() {
    let yaml = r#"
name: "two-checklists"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: check1
        type: checklist
        name: "Check 1"
        criteria: ["done"]
      - id: check2
        type: checklist
        name: "Check 2"
        criteria: ["done"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("exactly one checklist sub-state, found 2")),
        "{errs:?}"
    );
}

#[test]
fn rule3_checklist_not_last_fails() {
    let yaml = r#"
name: "checklist-not-last"
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
      - id: do_work
        type: execute
        name: "Do work"
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("checklist sub-state must be last")),
        "{errs:?}"
    );
}

#[test]
fn rule3_disabled_checklist_fails() {
    let yaml = r#"
name: "disabled-checklist"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        enabled: false
        criteria: ["done"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("checklist sub-state must not be disabled")),
        "{errs:?}"
    );
}

// ---------------------------------------------------------------------
// Rule 4: checklist criteria = Some(non-empty); non-checklist criteria = None.
// ---------------------------------------------------------------------

#[test]
fn rule4_checklist_missing_criteria_fails() {
    let yaml = r#"
name: "checklist-missing-criteria"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("checklist must have non-empty criteria")),
        "{errs:?}"
    );
}

#[test]
fn rule4_non_checklist_with_criteria_fails() {
    let yaml = r#"
name: "non-checklist-criteria"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: do_work
        type: execute
        name: "Do work"
        criteria: ["should not be here"]
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("non-checklist sub-state must not have criteria")),
        "{errs:?}"
    );
}

// ---------------------------------------------------------------------
// Rule 5: inject.skill / inject.protocol must resolve via the library.
// ---------------------------------------------------------------------

#[test]
fn rule5_resolved_skill_and_protocol_pass() {
    let yaml = r#"
name: "resolved-inject"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: greet
        type: inject
        name: "Greet"
        inject:
          skill: "greeting"
          protocol: "handoff"
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
"#;
    let profile = load(yaml);
    assert!(validate_profile(&profile, &fixture_library()).is_ok());
}

#[test]
fn rule5_unresolved_skill_fails() {
    let yaml = r#"
name: "unresolved-skill"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: greet
        type: inject
        name: "Greet"
        inject:
          skill: "does_not_exist"
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("unresolved skill 'does_not_exist'")),
        "{errs:?}"
    );
}

#[test]
fn rule5_unresolved_protocol_fails() {
    let yaml = r#"
name: "unresolved-protocol"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: greet
        type: inject
        name: "Greet"
        inject:
          protocol: "does_not_exist"
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("unresolved protocol 'does_not_exist'")),
        "{errs:?}"
    );
}

// ---------------------------------------------------------------------
// Rule 6: output_contract schema is valid Draft-7; destination has {session_id}.
// ---------------------------------------------------------------------

#[test]
fn rule6_valid_output_contract_passes() {
    let yaml = r#"
name: "valid-output-contract"
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
output_contract:
  format: "json"
  schema:
    type: "object"
  destination: "artifacts/{session_id}/out.json"
"#;
    let profile = load(yaml);
    assert!(validate_profile(&profile, &fixture_library()).is_ok());
}

#[test]
fn rule6_bad_output_contract_schema_fails() {
    let yaml = r#"
name: "bad-schema"
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
output_contract:
  format: "json"
  schema:
    type: 123
  destination: "artifacts/{session_id}/out.json"
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("output_contract.schema is not a valid JSON Schema")),
        "{errs:?}"
    );
}

#[test]
fn rule6_destination_missing_session_id_fails() {
    let yaml = r#"
name: "missing-session-id"
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
output_contract:
  format: "json"
  schema:
    type: "object"
  destination: "artifacts/out.json"
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("output_contract.destination must contain {session_id}")),
        "{errs:?}"
    );
}

// Rule 6 path-safety (FIX 1) and Rule 12 colon-charset (issue #69) live in
// `profile_validator_security_tests.rs`, split out to keep this file under the
// LOC cap (`scripts/check_loc.sh`).

// ---------------------------------------------------------------------
// Rule 7: non-lexical macro order is a warning only, not a violation.
// ---------------------------------------------------------------------

#[test]
fn rule7_non_lexical_macro_order_is_not_a_violation() {
    let yaml = r#"
name: "non-lexical-order"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: zeta
    name: "Zeta"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["done"]
"#;
    let profile = load(yaml);
    assert!(validate_profile(&profile, &fixture_library()).is_ok());
}

// ---------------------------------------------------------------------
// Rule 8: every verify check targets a declared criterion.
// ---------------------------------------------------------------------

#[test]
fn rule8_verify_targeting_declared_criterion_passes() {
    let yaml = r#"
name: "verify-ok"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["tests_pass"]
        verify:
          - criterion: "tests_pass"
            command: "cargo test"
"#;
    let profile = load(yaml);
    assert!(validate_profile(&profile, &fixture_library()).is_ok());
}

#[test]
fn rule8_verify_criterion_typo_fails() {
    // `tests_pas` is a typo for `tests_pass`; without this rule the verify
    // would be a silent no-op and the LLM's unverified claim would satisfy
    // `tests_pass`. The validator must catch it.
    let yaml = r#"
name: "verify-typo"
version: "1.0.0"
description: "d"
pipeline:
  - state_id: alpha
    name: "Alpha"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria: ["tests_pass"]
        verify:
          - criterion: "tests_pas"
            command: "cargo test"
"#;
    let profile = load(yaml);
    let errs = validate_profile(&profile, &fixture_library()).expect_err("must fail");
    assert!(
        errs.iter()
            .any(|e| e.contains("tests_pas") && e.contains("not in the sub-state's criteria")),
        "expected a Rule 8 violation naming the bad criterion, got: {errs:?}"
    );
}

// ---------------------------------------------------------------------
// default.yaml validates clean.
// ---------------------------------------------------------------------

#[test]
fn default_profile_validates_clean() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/default.yaml");
    let profile = load_profile(&path).expect("default.yaml should parse");
    let result = validate_profile(&profile, &fixture_library());
    assert!(
        result.is_ok(),
        "default.yaml should validate clean: {result:?}"
    );
}
