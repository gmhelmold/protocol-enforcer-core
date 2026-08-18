// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Lifecycle hook types (SPEC_plugins.md, WP-1) — additive only.
//!
//! These types describe executable extensions ("hooks") that can be
//! attached to profile states/sub-states. This module is purely data
//! shapes: no validation, no driver wiring. The strict
//! `manifest::validate_profile` and the fsm ignore `hooks` entirely
//! (passivity invariant, C1); wiring lands in later WPs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The lifecycle point a hook is bound to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HookEvent {
    PreMacroEnter,
    PreSubstateEnter,
    Verify,
    PostSubstateExit,
    PostMacroExit,
    OnReject,
    OnLoopBack,
}

/// The fail-policy class a `HookEvent` belongs to (SPEC_plugins.md, "Fail
/// policy (C3)"): the axis is the event class, not the hook kind.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HookEventClass {
    Verify,
    PreDrive,
    Advisory,
}

impl HookEvent {
    pub fn class(&self) -> HookEventClass {
        match self {
            HookEvent::Verify => HookEventClass::Verify,
            HookEvent::PreMacroEnter | HookEvent::PreSubstateEnter => HookEventClass::PreDrive,
            HookEvent::PostSubstateExit
            | HookEvent::PostMacroExit
            | HookEvent::OnReject
            | HookEvent::OnLoopBack => HookEventClass::Advisory,
        }
    }
}

/// Whether a hook validates (checks, never mutates) or mutates (may patch
/// evidence / inject context).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookKind {
    Validate,
    Mutate,
}

/// A hook definition, as stored in the hook library (`LibKind::Hook`) or
/// inlined directly into a `HookRef`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDef {
    pub id: String,
    pub version: String,
    pub kind: HookKind,
    pub events: Vec<HookEvent>,
    /// JSON-contract command (protocol 2).
    pub command: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_output_cap")]
    pub output_cap_bytes: usize,
    #[serde(default)]
    pub fail_open: Option<bool>,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// The checklist criteria (or completion-model scratch-keys) this hook is
    /// responsible for attesting when it is bound to the `verify` event.
    /// Declared up front so the driver can be fail-closed: on hook failure
    /// (or a criterion the hook did not attest), the driver strips exactly
    /// these keys from the evidence, so the checklist gate rejects. Must be
    /// non-empty for a `verify`-bound hook and empty otherwise (enforced by
    /// `validate_hooks`, Rule H7, amended 2026-08-13). Every entry must be
    /// EITHER one of the checklist sub-state's declared `criteria`, OR a
    /// scratch-key consumed by some same-sub-state **verify-bound** hook's
    /// `candidates` allowlist (`args["candidates"]`, comma-separated) — the
    /// completion-model pattern where a strip shrinks an admitted subset
    /// without failing the aggregate checklist criterion. Only verify-bound
    /// consumers count, so a decoy non-verify hook cannot launder a dead key.
    #[serde(default)]
    pub verify_criteria: Vec<String>,
}

fn default_timeout_ms() -> u64 {
    10_000
}

fn default_output_cap() -> usize {
    1024 * 1024
}

/// A reference to a hook attached to a `StateDef`/`SubStateDef`, either by
/// library id (`id`) or inline (`inline`) — exactly one of the two
/// (enforced later, by `validate_hooks`, Rule H1; not here).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRef {
    /// Library id; xor `inline`.
    #[serde(default)]
    pub id: Option<String>,
    /// Must match the library `HookDef.version` (Rule H4).
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub args: BTreeMap<String, String>,
    #[serde(default)]
    pub inline: Option<HookDef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_def_round_trips_through_serde() {
        let def = HookDef {
            id: "check-tests".to_string(),
            version: "1.0.0".to_string(),
            kind: HookKind::Validate,
            events: vec![HookEvent::PreSubstateEnter, HookEvent::Verify],
            command: "run-checks.sh".to_string(),
            timeout_ms: 5_000,
            output_cap_bytes: 2048,
            fail_open: Some(true),
            inputs: {
                let mut m = BTreeMap::new();
                m.insert("key".to_string(), "value".to_string());
                m
            },
            verify_criteria: vec!["tests_pass".to_string()],
        };

        let json = serde_json::to_string(&def).expect("serialize HookDef");
        let round_tripped: HookDef = serde_json::from_str(&json).expect("deserialize HookDef");

        assert_eq!(round_tripped.id, def.id);
        assert_eq!(round_tripped.verify_criteria, def.verify_criteria);
        assert_eq!(round_tripped.version, def.version);
        assert_eq!(round_tripped.kind, def.kind);
        assert_eq!(round_tripped.events, def.events);
        assert_eq!(round_tripped.command, def.command);
        assert_eq!(round_tripped.timeout_ms, def.timeout_ms);
        assert_eq!(round_tripped.output_cap_bytes, def.output_cap_bytes);
        assert_eq!(round_tripped.fail_open, def.fail_open);
        assert_eq!(round_tripped.inputs, def.inputs);
    }

    #[test]
    fn hook_ref_round_trips_through_serde() {
        let inline = HookDef {
            id: "inline-hook".to_string(),
            version: "0.1.0".to_string(),
            kind: HookKind::Mutate,
            events: vec![HookEvent::OnLoopBack],
            command: "do-thing.sh".to_string(),
            timeout_ms: default_timeout_ms(),
            output_cap_bytes: default_output_cap(),
            fail_open: None,
            inputs: BTreeMap::new(),
            verify_criteria: Vec::new(),
        };
        let href = HookRef {
            id: None,
            version: None,
            args: {
                let mut m = BTreeMap::new();
                m.insert("arg1".to_string(), "val1".to_string());
                m
            },
            inline: Some(inline),
        };

        let json = serde_json::to_string(&href).expect("serialize HookRef");
        let round_tripped: HookRef = serde_json::from_str(&json).expect("deserialize HookRef");

        assert_eq!(round_tripped.id, href.id);
        assert_eq!(round_tripped.version, href.version);
        assert_eq!(round_tripped.args, href.args);
        assert!(round_tripped.inline.is_some());
        assert_eq!(round_tripped.inline.unwrap().id, "inline-hook");
    }

    #[test]
    fn hook_event_class_maps_all_variants() {
        assert_eq!(HookEvent::Verify.class(), HookEventClass::Verify);
        assert_eq!(HookEvent::PreMacroEnter.class(), HookEventClass::PreDrive);
        assert_eq!(
            HookEvent::PreSubstateEnter.class(),
            HookEventClass::PreDrive
        );
        assert_eq!(
            HookEvent::PostSubstateExit.class(),
            HookEventClass::Advisory
        );
        assert_eq!(HookEvent::PostMacroExit.class(), HookEventClass::Advisory);
        assert_eq!(HookEvent::OnReject.class(), HookEventClass::Advisory);
        assert_eq!(HookEvent::OnLoopBack.class(), HookEventClass::Advisory);
    }

    #[test]
    fn hook_def_deserializes_with_defaults_when_timeout_and_cap_omitted() {
        let yaml = r#"
id: check-tests
version: "1.0.0"
kind: validate
events:
  - verify
command: run-checks.sh
"#;
        let def: HookDef = serde_yaml::from_str(yaml).expect("deserialize HookDef from yaml");
        assert_eq!(def.timeout_ms, 10_000);
        assert_eq!(def.output_cap_bytes, 1_048_576);
    }
}
