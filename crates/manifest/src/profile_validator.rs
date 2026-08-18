// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Strict nested-profile validator.
//!
//! `validate_profile` enforces the hard-fail rules the enforcer applies to
//! the served path (`protocol_start`). It is intentionally stricter than a
//! warn-only editor-side validator used elsewhere for authoring UX — both
//! operate over the same promoted types.

use protocol_library::{LibKind, Library};
use protocol_types::{Profile, StateDef, SubStateDef, SubStateType};

/// Validate `profile` against every structural rule (Rules 1-10), resolving `inject.skill`
/// / `inject.protocol` references against `lib`.
///
/// Returns `Ok(())` if the profile is valid, otherwise `Err(violations)`
/// with every rule violation found (not just the first).
pub fn validate_profile(profile: &Profile, lib: &Library) -> Result<(), Vec<String>> {
    let mut violations = Vec::new();

    // Rule 1: pipeline non-empty.
    if profile.pipeline.is_empty() {
        violations.push("pipeline must not be empty".to_string());
    }

    // Rule 2 (macro half): state_id unique across pipeline.
    check_duplicate_macro_ids(profile, &mut violations);

    for state in &profile.pipeline {
        validate_macro(state, lib, &mut violations);
    }

    // Macro-level `enabled`, normalized at load: at
    // least one macro must be enabled -- an all-disabled pipeline has no
    // effective normalized pipeline for the engine/driver to run.
    let normalized = profile.with_enabled_macros_only();
    if normalized.pipeline.is_empty() && !profile.pipeline.is_empty() {
        violations.push("at least one macro must be enabled".to_string());
    }

    // Rules 9 & 10: validate `loop_state` structurally,
    // independent of the global `auto_loop_on_checklist_failure` switch. Key
    // off the NORMALIZED pipeline (last ENABLED macro) so disabling the true
    // last macro correctly flags a now-final `loop: true` middle macro
    // (the CRITICAL case where normalization changes which macro is final)
    // -- structural per-macro checks
    // above already ran against the FULL profile, so a disabled macro must
    // still be well-formed on its own.
    check_loop_state(&normalized, &mut violations);

    // Rule 6: output_contract (if present) has a valid Draft-7 schema and a
    // destination containing the `{session_id}` placeholder.
    if let Some(contract) = &profile.output_contract {
        check_output_contract(contract, &mut violations);
    }

    // Rule 7: macro execution order is defined by pipeline array position,
    // not by state_id spelling — so there is nothing to warn about. (An
    // earlier build logged a "non-lexical macro order" warning that fired on
    // every semantically-ordered pipeline, e.g. understand→plan→execute; it
    // was pure noise and has been removed.)

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Rule 9: a macro with `loop_state == true` must contain at least one
/// enabled `Execute` sub-state -- the loop-back target.
/// Rule 10 (OWNER-DECIDED): `loop_state == true` is forbidden on the FINAL
/// macro (the last in the pipeline, whose checklist produces
/// `output_contract`) -- the delivery step must not cycle. Uses the shared
/// `StateDef::first_enabled_execute_index` (also used by the engine's
/// loop-back target resolution in `protocol-fsm`) so the two can never
/// drift.
fn check_loop_state(profile: &Profile, violations: &mut Vec<String>) {
    let last_macro_id = profile.pipeline.last().map(|m| m.state_id.as_str());
    for state in &profile.pipeline {
        if !state.loop_state {
            continue;
        }
        if state.first_enabled_execute_index().is_none() {
            violations.push(format!(
                "macro '{}': loop: true requires at least one enabled execute sub-state \
                 (the loop-back target), found none",
                state.state_id
            ));
        }
        if last_macro_id == Some(state.state_id.as_str()) {
            violations.push(format!(
                "macro '{}': loop: true is forbidden on the final pipeline macro \
                 (the delivery step must not cycle)",
                state.state_id
            ));
        }
    }
}

fn check_duplicate_macro_ids(profile: &Profile, violations: &mut Vec<String>) {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    for state in &profile.pipeline {
        if !seen.insert(state.state_id.as_str()) {
            violations.push(format!("duplicate macro state_id: {}", state.state_id));
        }
    }
}

fn validate_macro(state: &StateDef, lib: &Library, violations: &mut Vec<String>) {
    // Rule 12 (issue #69): reject ':' in the macro `state_id`. The approval
    // signature binds to `pe-approval-v1:<session>:<macro>:<sub_state>:<challenge>`
    // (`protocol_notary::approval`), a colon-delimited message whose
    // injection-safety rests on NO field containing the separator. `session_id`
    // is already colon-free (gateway allow-list); this mirrors that treatment
    // for the macro id so `approval.rs`'s "signature cannot be shifted" claim
    // holds for real.
    if state.state_id.contains(':') {
        violations.push(format!(
            "macro '{}': state_id must not contain ':' (reserved as the approval-signature field separator)",
            state.state_id
        ));
    }

    // Rule 1: each macro's sub_states non-empty.
    if state.sub_states.is_empty() {
        violations.push(format!(
            "macro '{}': sub_states must not be empty",
            state.state_id
        ));
        return;
    }

    // Rule 2 (sub half): sub `id` unique within a macro.
    check_duplicate_sub_ids(state, violations);

    // Rule 3: exactly one Checklist sub-state, last, not disabled.
    check_single_trailing_checklist(state, violations);

    for sub in &state.sub_states {
        // Rule 12 (issue #69): reject ':' in the sub-state `id`, same reason as
        // the macro id above — it is a colon-delimited field of the approval
        // signature message and must not carry the separator.
        if sub.id.contains(':') {
            violations.push(format!(
                "macro '{}' sub-state '{}': id must not contain ':' (reserved as the approval-signature field separator)",
                state.state_id, sub.id
            ));
        }
        // Rule 4: criteria presence/absence by type.
        check_criteria(state, sub, violations);
        // Rule 5: inject.skill / inject.protocol resolve via the library.
        check_injection(state, sub, lib, violations);
        // Rule 8: every verify check targets a declared criterion.
        check_verify(state, sub, violations);
        // Rule 11: approver_pubkey required iff human_approval.
        check_approval(state, sub, violations);
    }
}

/// A `human_approval` sub-state MUST carry a well-formed hex Ed25519
/// `approver_pubkey`, and no other sub-state type may carry one.
///
/// Both directions hard-fail, and for the same reason: the key is the ONLY
/// thing standing between "a human approved" and "the agent said so". Missing
/// or malformed, there is nothing to verify a signature against — a gate that
/// cannot be checked must not load rather than load and wave everything
/// through. Present on a non-approval sub-state, it is an author who believes
/// they built a gate and did not; failing loudly beats shipping the illusion.
fn check_approval(state: &StateDef, sub: &SubStateDef, violations: &mut Vec<String>) {
    let is_approval = sub.sub_state_type == SubStateType::HumanApproval;
    match (&sub.approver_pubkey, is_approval) {
        (Some(key), true) => {
            if !protocol_notary::is_valid_approver_pubkey(key) {
                violations.push(format!(
                    "macro '{}' sub-state '{}': approver_pubkey is not a valid 32-byte hex \
                     Ed25519 public key",
                    state.state_id, sub.id
                ));
            }
        }
        (None, true) => violations.push(format!(
            "macro '{}' sub-state '{}': human_approval sub-state requires approver_pubkey \
             (hex Ed25519 public key)",
            state.state_id, sub.id
        )),
        (Some(_), false) => violations.push(format!(
            "macro '{}' sub-state '{}': approver_pubkey is only allowed on a human_approval \
             sub-state (found on type '{}')",
            state.state_id, sub.id, sub.sub_state_type
        )),
        (None, false) => {}
    }
}

/// Rule 8: each `verify` check's `criterion` must be one of the sub-state's
/// declared `criteria`. A typo here would otherwise make the trusted-driver
/// verification a silent no-op (it would strip/insert a key nothing reads),
/// leaving the real criterion satisfiable by the LLM's unverified claim — the
/// exact fabrication hole the verify gate exists to close. The enforcer still
/// ignores `verify` at runtime; this is an authoring guard.
fn check_verify(state: &StateDef, sub: &SubStateDef, violations: &mut Vec<String>) {
    let Some(checks) = &sub.verify else {
        return;
    };
    let declared: &[String] = sub.criteria.as_deref().unwrap_or(&[]);
    for check in checks {
        if !declared.iter().any(|c| c == &check.criterion) {
            violations.push(format!(
                "macro '{}' sub-state '{}': verify check targets criterion '{}' \
                 which is not in the sub-state's criteria",
                state.state_id, sub.id, check.criterion
            ));
        }
    }
}

fn check_duplicate_sub_ids(state: &StateDef, violations: &mut Vec<String>) {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    for sub in &state.sub_states {
        if !seen.insert(sub.id.as_str()) {
            violations.push(format!(
                "macro '{}': duplicate sub-state id: {}",
                state.state_id, sub.id
            ));
        }
    }
}

fn check_single_trailing_checklist(state: &StateDef, violations: &mut Vec<String>) {
    let checklist_indices: Vec<usize> = state
        .sub_states
        .iter()
        .enumerate()
        .filter(|(_, s)| s.sub_state_type == SubStateType::Checklist)
        .map(|(i, _)| i)
        .collect();

    match checklist_indices.len() {
        1 => {
            let idx = checklist_indices[0];
            let last = state.sub_states.len() - 1;
            if idx != last {
                violations.push(format!(
                    "macro '{}': checklist sub-state must be last",
                    state.state_id
                ));
            }
            if !state.sub_states[idx].enabled {
                violations.push(format!(
                    "macro '{}': checklist sub-state must not be disabled",
                    state.state_id
                ));
            }
        }
        0 => violations.push(format!(
            "macro '{}': must have exactly one checklist sub-state, found none",
            state.state_id
        )),
        n => violations.push(format!(
            "macro '{}': must have exactly one checklist sub-state, found {n}",
            state.state_id
        )),
    }
}

fn check_criteria(state: &StateDef, sub: &SubStateDef, violations: &mut Vec<String>) {
    match sub.sub_state_type {
        SubStateType::Checklist => {
            let non_empty = matches!(&sub.criteria, Some(c) if !c.is_empty());
            if !non_empty {
                violations.push(format!(
                    "macro '{}' sub-state '{}': checklist must have non-empty criteria",
                    state.state_id, sub.id
                ));
            }
        }
        _ => {
            if sub.criteria.is_some() {
                violations.push(format!(
                    "macro '{}' sub-state '{}': non-checklist sub-state must not have criteria",
                    state.state_id, sub.id
                ));
            }
        }
    }
}

fn check_output_contract(contract: &protocol_types::OutputContract, violations: &mut Vec<String>) {
    if let Err(e) = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .build(&contract.schema)
    {
        violations.push(format!(
            "output_contract.schema is not a valid JSON Schema: {e}"
        ));
    }

    if !contract.destination.contains("{session_id}") {
        violations.push("output_contract.destination must contain {session_id}".to_string());
    }

    // Path-safety: the destination is written by the enforcer on the served
    // path, so a malicious profile must not be able to steer that write outside
    // the sandbox. A live exploit staged a profile whose destination was an
    // absolute path and wrote a file OUTSIDE the process cwd. Reject the three
    // ways a destination escapes: absolute path, a `..` parent-hop component,
    // and an embedded NUL (which truncates the path at the syscall boundary).
    // The engine (`validate_and_write_output`) confines the write as defense in
    // depth; this is the first, load-time gate.
    if contract.destination.contains('\0') {
        violations.push("output_contract.destination must not contain a NUL byte".to_string());
    }
    let dest_path = std::path::Path::new(&contract.destination);
    if dest_path.is_absolute() {
        violations.push(format!(
            "output_contract.destination must be a relative path, got absolute '{}'",
            contract.destination
        ));
    }
    if dest_path
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        violations.push(format!(
            "output_contract.destination must not contain a '..' path component: '{}'",
            contract.destination
        ));
    }
}

fn check_injection(
    state: &StateDef,
    sub: &SubStateDef,
    lib: &Library,
    violations: &mut Vec<String>,
) {
    let Some(inject) = &sub.inject else {
        return;
    };

    if let Some(skill) = &inject.skill {
        if lib.resolve(LibKind::Skill, skill).is_err() {
            violations.push(format!(
                "macro '{}' sub-state '{}': unresolved skill '{}'",
                state.state_id, sub.id, skill
            ));
        }
    }

    if let Some(protocol) = &inject.protocol {
        if lib.resolve(LibKind::Protocol, protocol).is_err() {
            violations.push(format!(
                "macro '{}' sub-state '{}': unresolved protocol '{}'",
                state.state_id, sub.id, protocol
            ));
        }
    }
}
