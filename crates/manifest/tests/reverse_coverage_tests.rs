// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Unit tests for `protocol_manifest::validate_hooks` rule H-RC (reverse
//! coverage), the dual of H7. H7 proves the FORWARD
//! inclusion: every `verify_criteria` entry is a declared checklist
//! criterion. H-RC proves the REVERSE inclusion: when a checklist sub-state
//! has ≥1 verify-bound hook, EVERY declared criterion must be covered by the
//! union of `verify_criteria` across that sub-state's verify hooks — else an
//! admit-gating criterion would pass unchecked (the authoring-time twin of
//! the runtime forge hole H1c closed with an allowlist+nonce).
//!
//! Scope discipline: H-RC fires ONLY when the sub-state already has ≥1
//! verify hook. A checklist with zero verify hooks is the legacy advisory
//! regime and is left untouched (RC-scoped-off below).

use std::collections::BTreeMap;
use std::path::PathBuf;

use protocol_library::Library;
use protocol_manifest::validate_hooks;
use protocol_types::hooks::{HookDef, HookEvent, HookKind, HookRef};
use protocol_types::profile::{Profile, ProfileSettings, StateDef, SubStateDef, SubStateType};

fn empty_library() -> Library {
    Library::new(PathBuf::from("/does/not/exist"))
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

/// A single-macro, single-checklist-sub-state profile with the given
/// `criteria` and `hooks` attached to that sub-state.
fn checklist_profile(criteria: Vec<&str>, sub_hooks: Vec<HookRef>) -> Profile {
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
                sub_state_type: SubStateType::Checklist,
                name: "Sub 1".to_string(),
                description: String::new(),
                enabled: true,
                criteria: Some(criteria.into_iter().map(String::from).collect()),
                inject: None,
                verify: None,
                approver_pubkey: None,
                approval_prompt: None,
                hooks: sub_hooks,
            }],
            hooks: vec![],
        }],
        output_contract: None,
    }
}

// -- RC-catches-forge (the teeth) -------------------------------------------

#[test]
fn rc_catches_uncovered_criterion() {
    // Checklist declares two criteria, but the sole verify hook only
    // attests one -- 'other' is admit-gating yet never oracle-checked.
    let mut def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    def.verify_criteria = vec!["done".to_string()];
    let profile = checklist_profile(vec!["done", "other"], vec![href_inline(def)]);

    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(
        err.iter().any(|v| v.contains("'other'")
            && v.contains("sub-state 'sub1'")
            && v.contains("no verify hook's")),
        "expected an H-RC violation naming criterion 'other' and sub-state 'sub1', got: {err:?}"
    );
}

// -- RC-scoped-off: zero verify hooks leaves the legacy regime untouched ----

#[test]
fn rc_scoped_off_when_no_verify_hooks() {
    // Same "uncovered" criteria shape, but the sub-state has NO verify hook
    // at all (only a non-verify hook) -- legacy advisory regime, must NOT
    // produce an RC violation.
    let def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::PreSubstateEnter]);
    let profile = checklist_profile(vec!["done", "other"], vec![href_inline(def)]);

    assert!(
        validate_hooks(&profile, &empty_library()).is_ok(),
        "a checklist with zero verify hooks must not trigger H-RC"
    );
}

#[test]
fn rc_scoped_off_when_checklist_has_no_hooks_at_all() {
    let profile = checklist_profile(vec!["done", "other"], vec![]);
    assert!(
        validate_hooks(&profile, &empty_library()).is_ok(),
        "a checklist with no hooks at all must not trigger H-RC"
    );
}

// -- H7-still-holds: RC did not replace or weaken H7 ------------------------

#[test]
fn h7_still_rejects_verify_criteria_outside_checklist() {
    let mut def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    def.verify_criteria = vec!["nonexistent".to_string()]; // not in declared criteria
    let profile = checklist_profile(vec!["done"], vec![href_inline(def)]);

    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(
        err.iter().any(|v| v.contains(
            "is neither one of the checklist's declared criteria nor a scratch-key consumed"
        )),
        "H7 must still fire: {err:?}"
    );
}

// -- RC passes when fully covered (positive control, single hook) ----------

#[test]
fn rc_passes_when_single_hook_covers_all_criteria() {
    let mut def = valid_inline_def("v", HookKind::Mutate, vec![HookEvent::Verify]);
    def.verify_criteria = vec!["done".to_string(), "other".to_string()];
    let profile = checklist_profile(vec!["done", "other"], vec![href_inline(def)]);

    assert!(validate_hooks(&profile, &empty_library()).is_ok());
}

// -- RC passes when coverage is the UNION across multiple verify hooks -----

#[test]
fn rc_passes_when_union_of_multiple_verify_hooks_covers_all_criteria() {
    let mut def_a = valid_inline_def("va", HookKind::Mutate, vec![HookEvent::Verify]);
    def_a.verify_criteria = vec!["done".to_string()];
    let mut def_b = valid_inline_def("vb", HookKind::Mutate, vec![HookEvent::Verify]);
    def_b.verify_criteria = vec!["other".to_string()];
    let profile = checklist_profile(
        vec!["done", "other"],
        vec![href_inline(def_a), href_inline(def_b)],
    );

    assert!(validate_hooks(&profile, &empty_library()).is_ok());
}

#[test]
fn rc_catches_uncovered_criterion_even_with_multiple_verify_hooks() {
    // Two verify hooks, union still misses 'third'.
    let mut def_a = valid_inline_def("va", HookKind::Mutate, vec![HookEvent::Verify]);
    def_a.verify_criteria = vec!["done".to_string()];
    let mut def_b = valid_inline_def("vb", HookKind::Mutate, vec![HookEvent::Verify]);
    def_b.verify_criteria = vec!["other".to_string()];
    let profile = checklist_profile(
        vec!["done", "other", "third"],
        vec![href_inline(def_a), href_inline(def_b)],
    );

    let err = validate_hooks(&profile, &empty_library()).expect_err("must reject");
    assert!(
        err.iter()
            .any(|v| v.contains("'third'") && v.contains("sub-state 'sub1'")),
        "expected H-RC violation naming 'third': {err:?}"
    );
}
