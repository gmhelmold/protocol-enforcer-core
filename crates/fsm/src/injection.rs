// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Injection rendering + checklist presence-rule helpers for
//! `ProfileFsmEngine` (SPEC_v3.md §2, §3, WP3). Split out of
//! `profile_engine.rs` to keep both files under the 600 LOC cap; these are
//! pure, stateless helpers with no `&mut self`.

use protocol_library::{LibKind, Library};
use protocol_types::{ChecklistEvidence, FsmError, Position, StateDef, SubStateDef};

/// A rendered injection for the sub-state the caller is now at (SPEC_v3
/// §2/§9). `skill_ref`/`protocol_ref` carry the *name* referenced by the
/// sub-state's `inject` (for traceability); `rendered` is the resolved,
/// concatenated content in fixed order prompt → skill → protocol → context.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedInjection {
    pub prompt: Option<String>,
    pub skill_ref: Option<String>,
    pub protocol_ref: Option<String>,
    pub context: Option<String>,
    pub rendered: String,
    /// The live challenge nonce for a `human_approval` sub-state
    /// (SPEC_human_approval.md), `None` on every other type. Rendering is pure
    /// and stateless, so this is filled in by the engine (which owns the
    /// session and the ledger) right after the render, not here.
    pub approval_challenge: Option<String>,
    /// The `human_approval` sub-state's `approval_prompt` — the text the agent
    /// relays to the human it is asking to sign.
    pub approval_prompt: Option<String>,
}

/// The presence rule (SPEC_v3 §3): present AND not `null`, `""`
/// (whitespace-only counts as empty), `[]`, or `{}`.
pub(crate) fn is_present(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::String(s) => !s.trim().is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

/// Every `criteria` entry not present-per-`is_present` in `evidence`
/// (SPEC_v3 §3).
pub(crate) fn missing_criteria(criteria: &[String], evidence: &ChecklistEvidence) -> Vec<String> {
    criteria
        .iter()
        .filter(|c| !evidence.get(c.as_str()).map(is_present).unwrap_or(false))
        .cloned()
        .collect()
}

/// First `enabled` sub-state's index. The Checklist sub can never be
/// disabled (SPEC §4 validation), so a valid profile always has one.
pub(crate) fn first_enabled_index(macro_def: &StateDef) -> Result<usize, FsmError> {
    macro_def
        .sub_states
        .iter()
        .position(|s| s.enabled)
        .ok_or_else(|| {
            FsmError::ManifestInvalid(format!(
                "macro '{}' has no enabled sub-state",
                macro_def.state_id
            ))
        })
}

/// First enabled sub-state's index of type `Execute` (SPEC_loopback.md: the
/// macro loop-back target). Thin wrapper over the shared
/// `StateDef::first_enabled_execute_index` (which also backs the
/// validator's Rule 9/10, see `protocol_types::StateDef` for why the
/// canonical logic lives there instead of here).
pub(crate) fn first_enabled_execute_index(macro_def: &StateDef) -> Option<usize> {
    macro_def.first_enabled_execute_index()
}

/// Resolve + render a macro's loop-back target (SPEC_loopback.md): the
/// macro's first enabled Execute sub-state. Returns the new `Position` and
/// its rendered injection. Callers (the engine) are expected to only invoke
/// this for a macro that validation already guaranteed has such a sub-state;
/// the `FsmError::ManifestInvalid` here is a defensive fallback, not the
/// primary enforcement point (that's `profile_validator` Rule 9).
pub(crate) fn render_loop_target(
    macro_def: &StateDef,
    library: &Library,
) -> Result<(Position, RenderedInjection), FsmError> {
    let idx = first_enabled_execute_index(macro_def).ok_or_else(|| {
        FsmError::ManifestInvalid(format!(
            "macro '{}' has loop-back enabled but no enabled execute sub-state",
            macro_def.state_id
        ))
    })?;
    let sub = &macro_def.sub_states[idx];
    let position = Position {
        macro_id: macro_def.state_id.clone(),
        sub_state_id: sub.id.clone(),
    };
    let injected = render_injection(sub, macro_def, library, None)?;
    Ok((position, injected))
}

/// Next enabled sub-state's index after `from_idx`. Falls through to the
/// trailing Checklist (always enabled) when nothing else is enabled between
/// `from_idx` and the end.
pub(crate) fn next_enabled_index(macro_def: &StateDef, from_idx: usize) -> usize {
    macro_def
        .sub_states
        .iter()
        .enumerate()
        .skip(from_idx + 1)
        .find(|(_, s)| s.enabled)
        .map(|(i, _)| i)
        .unwrap_or(macro_def.sub_states.len() - 1)
}

/// Merge `initial_context` (start_session only) into a sub-state's base
/// `inject.context`, string-appending its pretty-printed JSON. Resolves a
/// SPEC §3 ambiguity: the spec says "merge into that sub's rendered
/// context" without prescribing a shape; append is deterministic and lossless.
fn merge_initial_context(
    base: Option<String>,
    initial: Option<&serde_json::Value>,
) -> Option<String> {
    let rendered_initial = initial
        .filter(|v| !v.is_null())
        .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()));
    match (base, rendered_initial) {
        (Some(b), Some(i)) => Some(format!("{b}\n\n{i}")),
        (Some(b), None) => Some(b),
        (None, Some(i)) => Some(i),
        (None, None) => None,
    }
}

/// Render a sub-state's injection (SPEC_v3 §2): resolve `inject.skill` /
/// `inject.protocol` against the library, fall back to the macro's
/// `system_prompt` when the sub has no `inject`, and concatenate in fixed
/// order prompt → skill → protocol → context.
pub(crate) fn render_injection(
    sub: &SubStateDef,
    macro_def: &StateDef,
    library: &Library,
    initial_context: Option<&serde_json::Value>,
) -> Result<RenderedInjection, FsmError> {
    let inject = sub.inject.as_ref();

    let prompt = inject
        .and_then(|i| i.prompt.clone())
        .or_else(|| macro_def.system_prompt.clone());
    let skill_name = inject.and_then(|i| i.skill.clone());
    let protocol_name = inject.and_then(|i| i.protocol.clone());
    let base_context = inject.and_then(|i| i.context.clone());
    let context = merge_initial_context(base_context, initial_context);

    let skill_content = match &skill_name {
        Some(name) => Some(
            library
                .resolve(LibKind::Skill, name)
                .map_err(|e| FsmError::LibraryUnresolved(format!("skill '{name}': {e}")))?,
        ),
        None => None,
    };
    let protocol_content = match &protocol_name {
        Some(name) => Some(
            library
                .resolve(LibKind::Protocol, name)
                .map_err(|e| FsmError::LibraryUnresolved(format!("protocol '{name}': {e}")))?,
        ),
        None => None,
    };

    let mut parts: Vec<String> = Vec::new();
    if let Some(p) = &prompt {
        parts.push(p.clone());
    }
    if let Some(s) = &skill_content {
        parts.push(s.clone());
    }
    if let Some(p) = &protocol_content {
        parts.push(p.clone());
    }
    if let Some(c) = &context {
        parts.push(c.clone());
    }

    Ok(RenderedInjection {
        prompt,
        skill_ref: skill_name,
        protocol_ref: protocol_name,
        context,
        rendered: parts.join("\n\n"),
        approval_challenge: None,
        approval_prompt: sub.approval_prompt.clone(),
    })
}
