// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Unit tests for `protocol_manifest::validate_hooks`
//! (rules H1-H6). Hermetic: id-resolution cases use a fresh
//! `tempfile::TempDir` hook library; everything else uses inline defs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use protocol_library::Library;
use protocol_manifest::validate_hooks;
use protocol_types::hooks::{HookDef, HookEvent, HookKind, HookRef};
use protocol_types::profile::{Profile, ProfileSettings, StateDef, SubStateDef, SubStateType};

fn empty_library() -> Library {
    Library::new(PathBuf::from("/does/not/exist"))
}

/// A library rooted at a fresh temp dir with `hooks/<id>.yaml` written for
/// each `(id, def)` pair.
fn hook_library(entries: &[(&str, &HookDef)]) -> (tempfile::TempDir, Library) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let hooks_dir = dir.path().join("hooks");
    std::fs::create_dir_all(&hooks_dir).expect("mkdir hooks");
    for (id, def) in entries {
        let yaml = serde_yaml::to_string(def).expect("serialize HookDef");
        std::fs::write(hooks_dir.join(format!("{id}.yaml")), yaml).expect("write hook yaml");
    }
    let lib = Library::new(dir.path());
    (dir, lib)
}

fn valid_inline_def(id: &str, kind: HookKind, events: Vec<HookEvent>) -> HookDef {
    HookDef {
        id: id.to_string(),
        version: "1.0.0".to_string(),
        kind,
        events,
        command: "true".to_string(),
        timeout_ms: 10_000,
        output_cap_bytes: 1024,
        fail_open: None,
        inputs: BTreeMap::new(),
        verify_criteria: Vec::new(),
    }
}

fn href_inline(def: HookDef) -> HookRef {
    HookRef {
        id: None,
        version: None,
        args: BTreeMap::new(),
        inline: Some(def),
    }
}

fn href_id(id: &str, version: &str) -> HookRef {
    HookRef {
        id: Some(id.to_string()),
        version: Some(version.to_string()),
        args: BTreeMap::new(),
        inline: None,
    }
}

/// A single-macro profile: `phase1` with `sub_state_type` and `hooks` on
/// both the macro and the sub-state configurable by the caller.
fn profile_with(
    macro_hooks: Vec<HookRef>,
    sub_type: SubStateType,
    sub_hooks: Vec<HookRef>,
) -> Profile {
    let criteria = if sub_type == SubStateType::Checklist {
        Some(vec!["done".to_string()])
    } else {
        None
    };
    Profile {
        name: "fixture".to_string(),
        version: "1.0.0".to_string(),
        description: String::new(),
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
            sub_states: vec![SubStateDef {
                id: "sub1".to_string(),
                sub_state_type: sub_type,
                name: "Sub 1".to_string(),
                description: String::new(),
                enabled: true,
                criteria,
                inject: None,
                verify: None,
                approver_pubkey: None,
                approval_prompt: None,
                hooks: sub_hooks,
            }],
            hooks: macro_hooks,
        }],
        output_contract: None,
    }
}

// -- H1: exactly one of id / inline --------------------------------------

#[test]
fn h1_neither_id_nor_inline_is_rejected() {
    let href = HookRef {
        id: None,
        version: None,
        args: BTreeMap::new(),
        inline: None,
    };
    let profile = profile_with(vec![], SubStateType::Execute, vec![href]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err
        .iter()
        .any(|v| v.contains("exactly one of `id` or `inline`")));
}

#[test]
fn h1_both_id_and_inline_is_rejected() {
    let def = valid_inline_def("x", HookKind::Mutate, vec![HookEvent::PreMacroEnter]);
    let mut href = href_id("x", "1.0.0");
    href.inline = Some(def);
    let profile = profile_with(vec![], SubStateType::Execute, vec![href]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err
        .iter()
        .any(|v| v.contains("exactly one of `id` or `inline`")));
}

#[test]
fn h1_inline_only_passes() {
    let def = valid_inline_def("x", HookKind::Mutate, vec![HookEvent::PreMacroEnter]);
    let profile = profile_with(vec![href_inline(def)], SubStateType::Execute, vec![]);
    assert!(validate_hooks(&profile, &empty_library()).is_ok());
}

// -- H2: id resolution ----------------------------------------------------

#[test]
fn h2_unresolved_id_is_rejected() {
    let href = href_id("missing", "1.0.0");
    let profile = profile_with(vec![], SubStateType::Execute, vec![href]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err.iter().any(|v| v.contains("unresolved hook 'missing'")));
}

#[test]
fn h2_resolved_id_passes() {
    let def = valid_inline_def("known", HookKind::Mutate, vec![HookEvent::PreMacroEnter]);
    let (_dir, lib) = hook_library(&[("known", &def)]);
    let profile = profile_with(
        vec![href_id("known", "1.0.0")],
        SubStateType::Execute,
        vec![],
    );
    assert!(validate_hooks(&profile, &lib).is_ok());
}

// -- H3: verify event only on checklist sub-state --------------------------

#[test]
fn h3_verify_event_on_macro_is_rejected() {
    let def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    let profile = profile_with(vec![href_inline(def)], SubStateType::Execute, vec![]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err
        .iter()
        .any(|v| v.contains("only allowed on a checklist")));
}

#[test]
fn h3_verify_event_on_non_checklist_sub_is_rejected() {
    let def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    let profile = profile_with(vec![], SubStateType::Execute, vec![href_inline(def)]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err
        .iter()
        .any(|v| v.contains("only allowed on a checklist")));
}

#[test]
fn h3_verify_event_on_checklist_sub_passes() {
    let mut def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    def.verify_criteria = vec!["done".to_string()]; // H7: must declare its targets
    let profile = profile_with(vec![], SubStateType::Checklist, vec![href_inline(def)]);
    assert!(validate_hooks(&profile, &empty_library()).is_ok());
}

// -- H7: verify_criteria contract -----------------------------------------

#[test]
fn h7_verify_hook_without_criteria_is_rejected() {
    // Mutate + Verify on a checklist, but no verify_criteria declared.
    let def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    let profile = profile_with(vec![], SubStateType::Checklist, vec![href_inline(def)]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err
        .iter()
        .any(|v| v.contains("must declare a non-empty `verify_criteria`")));
}

#[test]
fn h7_verify_criteria_not_in_checklist_is_rejected() {
    // Amended H7 (2026-08-13, owner-ratified): a verify_criteria entry that
    // is neither a checklist criterion (fixture criteria = ["done"]) NOR a
    // scratch-key consumed by some hook's `candidates` allowlist on this
    // sub-state is STILL a violation — this is the teeth the amendment
    // keeps (the mutation "strip-target nothing reads" must die).
    let mut def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    def.verify_criteria = vec!["nonexistent".to_string()]; // fixture criteria = ["done"]
    let profile = profile_with(vec![], SubStateType::Checklist, vec![href_inline(def)]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err.iter().any(|v| v.contains(
        "is neither one of the checklist's declared criteria nor a scratch-key consumed"
    )));
}

#[test]
fn h7_amended_clause_b_scratch_key_consumed_by_candidates_allowlist_passes() {
    // Completion-model pattern: the verify hook's verify_criteria
    // are scratch-keys, NOT checklist criteria; a second hook on the SAME
    // sub-state declares them in its `candidates` allowlist arg — that is
    // the downstream consumption that makes the strip meaningful. Amended
    // H7 clause (b) must accept this.
    let mut verify_def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    verify_def.verify_criteria = vec!["cand_1".to_string(), "cand_2".to_string()];
    let verify_href = href_inline(verify_def);

    let mut attest_def = valid_inline_def("a", HookKind::Mutate, vec![HookEvent::Verify]);
    attest_def.verify_criteria = vec!["done".to_string()]; // the checklist criterion
    let mut attest_href = href_inline(attest_def);
    attest_href
        .args
        .insert("candidates".to_string(), "cand_1,cand_2".to_string());

    let profile = profile_with(
        vec![],
        SubStateType::Checklist,
        vec![verify_href, attest_href],
    );
    validate_hooks(&profile, &empty_library()).expect("clause (b) scratch-keys must pass");
}

#[test]
fn h7_amended_clause_b_unconsumed_scratch_key_still_rejected() {
    // Same shape as the pass case above, but NO hook on the sub-state
    // declares `cand_1`/`cand_2` in a `candidates` allowlist — the strip
    // targets nothing. Must still be a violation (the teeth).
    let mut verify_def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    verify_def.verify_criteria = vec!["cand_1".to_string(), "cand_2".to_string()];
    let verify_href = href_inline(verify_def);

    let mut attest_def = valid_inline_def("a", HookKind::Mutate, vec![HookEvent::Verify]);
    attest_def.verify_criteria = vec!["done".to_string()];
    let attest_href = href_inline(attest_def); // no `candidates` arg

    let profile = profile_with(
        vec![],
        SubStateType::Checklist,
        vec![verify_href, attest_href],
    );
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err.iter().any(|v| v.contains("cand_1")
        && v.contains(
            "is neither one of the checklist's declared criteria nor a scratch-key consumed"
        )));
}

#[test]
fn h7_amended_clause_b_decoy_non_verify_hook_cannot_launder_a_dead_key() {
    // DECOY HARDENING (soundness): clause (b) only counts `candidates`
    // declared by a VERIFY-bound hook. Here the verify hook declares
    // `cand_1` as a verify_criterion, and a NON-verify (advisory) decoy hook
    // declares `candidates: "cand_1"`. Because the decoy is not verify-bound,
    // it does NOT consume the scratch-key in the verify/admit pipeline — so
    // `cand_1` is still a dead strip-target and MUST be rejected. A syntactic
    // string-containment check (any hook's candidates) would wrongly pass this.
    let mut verify_def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    verify_def.verify_criteria = vec!["cand_1".to_string()];
    let verify_href = href_inline(verify_def);

    // Decoy: advisory PostSubstateExit event (NOT verify), declares candidates.
    let decoy_def = valid_inline_def("decoy", HookKind::Mutate, vec![HookEvent::PostSubstateExit]);
    let mut decoy_href = href_inline(decoy_def);
    decoy_href
        .args
        .insert("candidates".to_string(), "cand_1".to_string());

    let profile = profile_with(
        vec![],
        SubStateType::Checklist,
        vec![verify_href, decoy_href],
    );
    let err = validate_hooks(&profile, &empty_library())
        .expect_err("a decoy non-verify hook must not launder a dead verify_criteria key");
    assert!(err.iter().any(|v| v.contains("cand_1")
        && v.contains(
            "is neither one of the checklist's declared criteria nor a scratch-key consumed"
        )));
}

#[test]
fn h7_verify_criteria_on_non_verify_hook_is_rejected() {
    let mut def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::PreSubstateEnter]);
    def.verify_criteria = vec!["done".to_string()]; // inert on a non-verify hook
    let profile = profile_with(vec![], SubStateType::Checklist, vec![href_inline(def)]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err
        .iter()
        .any(|v| v.contains("verify_criteria is only meaningful on a `verify`-bound hook")));
}

// -- H4: version pin -------------------------------------------------------

#[test]
fn h4_version_mismatch_is_rejected() {
    let def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::PreMacroEnter]);
    let (_dir, lib) = hook_library(&[("v", &def)]);
    let profile = profile_with(vec![href_id("v", "2.0.0")], SubStateType::Execute, vec![]);
    let err = validate_hooks(&profile, &lib).expect_err("must reject");
    assert!(err
        .iter()
        .any(|e| e.contains("version pin mismatch: ref=2.0.0 lib=1.0.0")));
}

#[test]
fn h4_matching_version_passes() {
    let def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::PreMacroEnter]);
    let (_dir, lib) = hook_library(&[("v", &def)]);
    let profile = profile_with(vec![href_id("v", "1.0.0")], SubStateType::Execute, vec![]);
    assert!(validate_hooks(&profile, &lib).is_ok());
}

#[test]
fn h4_missing_version_is_rejected() {
    let def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::PreMacroEnter]);
    let (_dir, lib) = hook_library(&[("v", &def)]);
    let href = HookRef {
        id: Some("v".to_string()),
        version: None,
        args: BTreeMap::new(),
        inline: None,
    };
    let profile = profile_with(vec![href], SubStateType::Execute, vec![]);
    let err = validate_hooks(&profile, &lib).expect_err("must reject");
    assert!(err.iter().any(|e| e.contains("version pin mismatch")));
}

// -- H5: kind x event-class legality ----------------------------------------

#[test]
fn h5_validate_on_verify_is_rejected() {
    let def = valid_inline_def("v", HookKind::Validate, vec![HookEvent::Verify]);
    let profile = profile_with(vec![], SubStateType::Checklist, vec![href_inline(def)]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err
        .iter()
        .any(|e| e.contains("`validate` kind hook is forbidden on the `verify` event")));
}

#[test]
fn h5_validate_on_advisory_is_rejected() {
    let def = valid_inline_def("v", HookKind::Validate, vec![HookEvent::PostMacroExit]);
    let profile = profile_with(vec![href_inline(def)], SubStateType::Execute, vec![]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err
        .iter()
        .any(|e| e.contains("forbidden on advisory event")));
}

#[test]
fn h5_validate_on_pre_drive_passes() {
    let def = valid_inline_def(
        "v",
        HookKind::Validate,
        vec![HookEvent::PreMacroEnter, HookEvent::PreSubstateEnter],
    );
    let profile = profile_with(vec![href_inline(def)], SubStateType::Execute, vec![]);
    assert!(validate_hooks(&profile, &empty_library()).is_ok());
}

#[test]
fn h5_mutate_on_any_class_passes() {
    let def = valid_inline_def(
        "v",
        HookKind::Mutate,
        vec![HookEvent::PostMacroExit, HookEvent::PreMacroEnter],
    );
    let profile = profile_with(vec![href_inline(def)], SubStateType::Execute, vec![]);
    assert!(validate_hooks(&profile, &empty_library()).is_ok());

    let mut def_verify = valid_inline_def("w", HookKind::Mutate, vec![HookEvent::Verify]);
    def_verify.verify_criteria = vec!["done".to_string()]; // H7: verify hooks declare targets
    let profile2 = profile_with(
        vec![],
        SubStateType::Checklist,
        vec![href_inline(def_verify)],
    );
    assert!(validate_hooks(&profile2, &empty_library()).is_ok());
}

// -- H6: timeout_ms / output_cap_bytes must be > 0 --------------------------

#[test]
fn h6_zero_timeout_is_rejected_inline() {
    let mut def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::PreMacroEnter]);
    def.timeout_ms = 0;
    let profile = profile_with(vec![href_inline(def)], SubStateType::Execute, vec![]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err.iter().any(|e| e.contains("timeout_ms must be > 0")));
}

#[test]
fn h6_zero_output_cap_is_rejected_inline() {
    let mut def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::PreMacroEnter]);
    def.output_cap_bytes = 0;
    let profile = profile_with(vec![href_inline(def)], SubStateType::Execute, vec![]);
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(err
        .iter()
        .any(|e| e.contains("output_cap_bytes must be > 0")));
}

#[test]
fn h6_zero_timeout_is_rejected_for_resolved_library_def() {
    let mut def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::PreMacroEnter]);
    def.timeout_ms = 0;
    let (_dir, lib) = hook_library(&[("v", &def)]);
    let profile = profile_with(vec![href_id("v", "1.0.0")], SubStateType::Execute, vec![]);
    let err = validate_hooks(&profile, &lib).expect_err("must reject");
    assert!(err.iter().any(|e| e.contains("timeout_ms must be > 0")));
}

#[test]
fn h6_positive_values_pass() {
    let def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::PreMacroEnter]);
    let profile = profile_with(vec![href_inline(def)], SubStateType::Execute, vec![]);
    assert!(validate_hooks(&profile, &empty_library()).is_ok());
}

// -- Aggregate: multiple violations collected in one pass -------------------

#[test]
fn collects_all_violations_not_just_first() {
    let bad_neither = HookRef {
        id: None,
        version: None,
        args: BTreeMap::new(),
        inline: None,
    };
    let bad_unresolved = href_id("nope", "1.0.0");
    let profile = profile_with(
        vec![bad_neither, bad_unresolved],
        SubStateType::Execute,
        vec![],
    );
    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert_eq!(err.len(), 2, "expected both violations: {err:?}");
}

// -- Passivity: no hook rule ever reachable from `validate_profile` --------

#[test]
fn validate_profile_ignores_hooks_entirely() {
    // A profile whose only defect is a hook-rule violation (H2: unresolved
    // id) must pass the STRICT served-path validator untouched -- proving
    // `validate_profile` never looks at `hooks` (passivity invariant, C1).
    // Structurally otherwise valid: one macro, trailing checklist sub-state.
    let href = href_id("definitely-does-not-exist", "1.0.0");
    let profile = profile_with(vec![href], SubStateType::Checklist, vec![]);

    // Sanity check: this profile IS invalid by hook rules (H2).
    assert!(validate_hooks(&profile, &empty_library()).is_err());

    // But the strict served-path validator never looks at `hooks` at all.
    assert!(
        protocol_manifest::validate_profile(&profile, &empty_library()).is_ok(),
        "validate_profile must ignore hooks entirely (passivity invariant, C1)"
    );
}
