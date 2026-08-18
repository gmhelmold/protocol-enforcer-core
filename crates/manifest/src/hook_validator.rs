// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! `validate_hooks` — where hook validation lives, and why it is kept
//! separate from the served-path validator.
//!
//! CRITICAL PASSIVITY CONSTRAINT (rule C1): this module is NEVER
//! called from the strict served-path validator `profile_validator::
//! validate_profile` (in turn called by the enforcer's
//! `ProfileFsmEngine::start_session`). A profile with `hooks` and the same
//! profile with `hooks` stripped MUST produce byte-identical enforcer
//! behavior. `validate_hooks` is a SEPARATE entry point the CLI
//! (`ProfileEditor` hook ops) and the driver call at load/write time — never
//! on the served path.

use protocol_library::Library;
use protocol_types::hooks::{HookDef, HookEvent, HookEventClass, HookKind, HookRef};
use protocol_types::{Profile, SubStateType};

/// Validate every `HookRef` attached to every macro (`StateDef.hooks`) and
/// sub-state (`SubStateDef.hooks`) in `profile` against rules H1-H6.
/// Collects ALL violations
/// rather than stopping at the first.
pub fn validate_hooks(profile: &Profile, lib: &Library) -> Result<(), Vec<String>> {
    let mut violations = Vec::new();

    for state in &profile.pipeline {
        let macro_loc = format!("macro '{}'", state.state_id);
        for href in &state.hooks {
            // A macro is never a checklist sub-state, so a `verify`-bound
            // hook is never legal here (H3): pass `None` for its criteria.
            validate_hook_ref(
                &macro_loc,
                href,
                None,
                &std::collections::HashSet::new(),
                lib,
                &mut violations,
            );
        }

        for sub in &state.sub_states {
            let sub_loc = format!("macro '{}' sub-state '{}'", state.state_id, sub.id);
            // `Some(criteria)` iff this is a checklist sub-state; that both
            // gates H3 and gives H7 the set a `verify_criteria` must be a
            // subset of. A checklist with no `criteria` yields an empty slice.
            let checklist_criteria: Option<&[String]> =
                if sub.sub_state_type == SubStateType::Checklist {
                    Some(sub.criteria.as_deref().unwrap_or(&[]))
                } else {
                    None
                };
            // H7 clause (b): the union of every same-sub-state hook's parsed
            // `candidates` allowlist (an `args` value, comma-separated) — but
            // only from VERIFY-bound hooks (decoy hardening). A verify_criteria
            // entry that is not a checklist criterion but IS in this set is a
            // scratch-key some verify hook on this sub-state reads downstream
            // (the completion-model strip-then-admit pattern) — legitimate, not dead.
            let candidate_scratch_keys = collect_candidate_scratch_keys(&sub.hooks, lib);

            for href in &sub.hooks {
                validate_hook_ref(
                    &sub_loc,
                    href,
                    checklist_criteria,
                    &candidate_scratch_keys,
                    lib,
                    &mut violations,
                );
            }

            // H-RC (reverse coverage): the dual of H7. H7 proves the FORWARD
            // inclusion (every verify_criteria entry is a declared criterion);
            // this proves the REVERSE inclusion (every declared criterion is
            // covered by at least one verify hook), but ONLY when the
            // sub-state already has ≥1 verify-bound hook — a checklist with
            // zero verify hooks is the legacy advisory regime and is left
            // untouched. Scoped to Checklist sub-states via `checklist_criteria`.
            if let Some(criteria) = checklist_criteria {
                let verify_defs: Vec<HookDef> = sub
                    .hooks
                    .iter()
                    .filter_map(|href| resolve_effective_def(href, lib))
                    .filter(|def| def.events.contains(&HookEvent::Verify))
                    .collect();
                if !verify_defs.is_empty() {
                    let covered: std::collections::HashSet<&str> = verify_defs
                        .iter()
                        .flat_map(|def| def.verify_criteria.iter().map(String::as_str))
                        .collect();
                    for criterion in criteria {
                        if !covered.contains(criterion.as_str()) {
                            violations.push(format!(
                                "{sub_loc}: checklist criterion '{criterion}' is verified \
                                 (sub-state has ≥1 verify hook) but no verify hook's \
                                 `verify_criteria` covers it — it would pass admit unchecked"
                            ));
                        }
                    }
                }
            }
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Collect the union of every VERIFY-bound hook's parsed `candidates` allowlist
/// on a single sub-state, for amended-H7 clause (b). `candidates` is a hook
/// `args` value: a comma-separated string of scratch-keys (e.g.
/// `"dep_candidate_1,dep_candidate_2"`). A hook with no `candidates` arg — or a
/// non-verify-bound hook (decoy hardening, see body) — contributes nothing;
/// entries are trimmed; empty entries are dropped.
fn collect_candidate_scratch_keys(
    hooks: &[HookRef],
    lib: &Library,
) -> std::collections::HashSet<String> {
    hooks
        .iter()
        // SOUNDNESS (decoy hardening): only a VERIFY-bound hook's `candidates`
        // launders a scratch-key past H7 clause (b). Otherwise a decoy advisory
        // /fail-open hook could declare `candidates: "forged_key"` purely to slip
        // a dead `verify_criteria` entry past the gate without that key ever being
        // consumed by the verify/admit pipeline. Requiring the consuming hook to be
        // verify-bound keeps the "strip is honored by a real reader" contract.
        .filter(|href| {
            resolve_effective_def(href, lib)
                .map(|def| def.events.contains(&HookEvent::Verify))
                .unwrap_or(false)
        })
        .filter_map(|href| href.args.get("candidates"))
        .flat_map(|raw| raw.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Resolve a `HookRef` to its effective `HookDef` (inline, or looked up in
/// `lib` by `id`) without emitting any violations — used by H-RC to gather
/// the union of `verify_criteria` across a sub-state's verify hooks. Mirrors
/// the def-selection logic in `validate_hook_ref` (H1/H2), but silently
/// returns `None` on any malformed/unresolved ref since those are already
/// reported by `validate_hook_ref` itself.
fn resolve_effective_def(href: &HookRef, lib: &Library) -> Option<HookDef> {
    if href.id.is_some() == href.inline.is_some() {
        return None; // H1 violation, already reported elsewhere.
    }
    if let Some(inline) = &href.inline {
        return Some(inline.clone());
    }
    let id = href.id.as_ref()?;
    lib.resolve_hook(id).ok()
}

/// Validate one `HookRef` at `loc` (a human-readable location prefix for
/// violation messages). `checklist_criteria` is `Some(criteria)` iff the
/// attach point is a Checklist sub-state (its declared criteria, possibly
/// empty), and `None` otherwise — it drives both H3 and H7.
fn validate_hook_ref(
    loc: &str,
    href: &HookRef,
    checklist_criteria: Option<&[String]>,
    candidate_scratch_keys: &std::collections::HashSet<String>,
    lib: &Library,
    violations: &mut Vec<String>,
) {
    // H1: exactly one of `id` / `inline`.
    let has_id = href.id.is_some();
    let has_inline = href.inline.is_some();
    if has_id == has_inline {
        violations.push(format!(
            "{loc}: hook ref must set exactly one of `id` or `inline` \
             (has_id={has_id} has_inline={has_inline})"
        ));
        return;
    }

    // H2: an `id` ref must resolve in the hook library.
    let resolved: Option<HookDef> = match &href.id {
        Some(id) => match lib.resolve_hook(id) {
            Ok(def) => Some(def),
            Err(_) => {
                violations.push(format!("{loc}: unresolved hook '{id}'"));
                None
            }
        },
        None => None,
    };

    // H4: version pin (only meaningful when `id` resolved).
    if let (Some(id), Some(def)) = (&href.id, &resolved) {
        match &href.version {
            Some(v) if v == &def.version => {}
            Some(v) => violations.push(format!(
                "{loc}: hook '{id}' version pin mismatch: ref={v} lib={}",
                def.version
            )),
            None => violations.push(format!(
                "{loc}: hook '{id}' version pin mismatch: ref=None lib={}",
                def.version
            )),
        }
    }

    // The effective def to check H3/H5/H6 against: inline, or the resolved
    // library def. If `id` failed to resolve there is nothing further to
    // check (H2 already reported it).
    let def: Option<&HookDef> = if has_inline {
        href.inline.as_ref()
    } else {
        resolved.as_ref()
    };
    let Some(def) = def else {
        return;
    };

    // H3: a hook bound to `verify` is only allowed on a Checklist sub-state.
    let is_verify_hook = def.events.contains(&HookEvent::Verify);
    if is_verify_hook && checklist_criteria.is_none() {
        violations.push(format!(
            "{loc}: hook bound to the `verify` event is only allowed on a checklist \
             sub-state (never a macro, never a non-checklist sub-state)"
        ));
    }

    // H7 (amended 2026-08-13, owner-ratified — completion-model scratch-keys):
    // a `verify`-bound hook MUST declare a non-empty `verify_criteria`; each
    // entry `v` must satisfy AT LEAST ONE of:
    //   (a) `v` is a checklist criterion of the same sub-state (the original
    //       rule — the up-front target set the driver strips on failure, its
    //       fail-closed contract), OR
    //   (b) `v` appears in the parsed `candidates` allowlist of SOME hook on
    //       the SAME sub-state (a scratch-key consumed downstream — the
    //       completion-model pattern where a verify hook proves/strips
    //       per-candidate scratch-keys and a later hook's `candidates`
    //       allowlist admits exactly the surviving ones; only the aggregate
    //       checklist criterion gates completion).
    // An entry that is NEITHER is a strip-target nothing reads — dead and
    // forge-prone — and remains a violation (the teeth this amendment keeps).
    // A hook NOT bound to `verify` must leave `verify_criteria` empty (it
    // would be inert noise).
    if is_verify_hook {
        if def.verify_criteria.is_empty() {
            violations.push(format!(
                "{loc}: hook bound to the `verify` event must declare a non-empty \
                 `verify_criteria` (the criteria it attests, stripped on failure)"
            ));
        } else if let Some(criteria) = checklist_criteria {
            for target in &def.verify_criteria {
                let is_checklist_criterion = criteria.contains(target);
                let is_consumed_scratch_key = candidate_scratch_keys.contains(target);
                if !is_checklist_criterion && !is_consumed_scratch_key {
                    violations.push(format!(
                        "{loc}: verify_criteria entry '{target}' is neither one of the \
                         checklist's declared criteria nor a scratch-key consumed by \
                         another hook's `candidates` allowlist on this sub-state"
                    ));
                }
            }
        }
    } else if !def.verify_criteria.is_empty() {
        violations.push(format!(
            "{loc}: verify_criteria is only meaningful on a `verify`-bound hook \
             (this hook is not bound to the `verify` event)"
        ));
    }

    // H5: kind x event-class legality. `validate` is forbidden on `verify`
    // and on any `Advisory`-class event (only allowed on `PreDrive`).
    // `mutate` is allowed on any class.
    if def.kind == HookKind::Validate {
        for event in &def.events {
            match event.class() {
                HookEventClass::Verify => violations.push(format!(
                    "{loc}: `validate` kind hook is forbidden on the `verify` event"
                )),
                HookEventClass::Advisory => violations.push(format!(
                    "{loc}: `validate` kind hook is forbidden on advisory event {event:?} \
                     (validate is only allowed on pre-drive events)"
                )),
                HookEventClass::PreDrive => {}
            }
        }
    }

    // H6: guardrail values must be positive.
    if def.timeout_ms == 0 {
        violations.push(format!("{loc}: hook timeout_ms must be > 0"));
    }
    if def.output_cap_bytes == 0 {
        violations.push(format!("{loc}: hook output_cap_bytes must be > 0"));
    }
}
