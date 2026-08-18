// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Profile system - data-driven state machine configuration
//!
//! Promoted from `protocol-agent` (WP0, SPEC_v3 §1/§1.1) so the profile
//! model is a shared type available to the fsm/gateway crates without a
//! dependency on the agent crate. `ProfileManager` (CRUD over the
//! filesystem) stays in `protocol-agent`; this module holds only the
//! serde-facing data shapes.

use crate::common::OutputContract;
use serde::{Deserialize, Serialize};

/// Profile definition (loaded from YAML)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub protected: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub cloned_from: Option<String>,
    #[serde(default)]
    pub settings: ProfileSettings,
    pub pipeline: Vec<StateDef>,
    #[serde(default)]
    pub output_contract: Option<OutputContract>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSettings {
    /// Consecutive-identical-rejection threshold for the `RepetitiveLoop`
    /// circuit breaker. Read by `ProfileFsmEngine::reject_checklist`.
    #[serde(default = "default_oscillation")]
    pub oscillation_detection_threshold: u32,
    /// Master switch for macro loop-back (SPEC_loopback.md); AND-gated with a
    /// macro's `loop_state`. Read by `ProfileFsmEngine::reject_checklist`.
    #[serde(default = "default_auto_loop")]
    pub auto_loop_on_checklist_failure: bool,
    /// Wall-clock budget for the whole session, in seconds; `0` disables the
    /// `GlobalTimeout` circuit breaker. Seeds `FsmConfig::global_timeout_seconds`
    /// when the gateway starts or recovers a session.
    #[serde(default = "default_global_timeout")]
    pub global_timeout_seconds: u64,
    /// Max consecutive rejected `human_approval` submissions allowed at one gate
    /// entry before the `ApprovalRejectionLimit` circuit breaker trips
    /// (security-audit FIX 3, issue #70). Read by
    /// `ProfileFsmEngine::step_approval`. A VALID signature always advances and
    /// resets the count; this only bounds the reject path so an agent cannot
    /// grow the `ApprovalRejected` ledger without limit.
    #[serde(default = "default_approval_rejection_cap")]
    pub approval_rejection_cap: u32,
}

impl Default for ProfileSettings {
    fn default() -> Self {
        Self {
            oscillation_detection_threshold: default_oscillation(),
            auto_loop_on_checklist_failure: default_auto_loop(),
            global_timeout_seconds: default_global_timeout(),
            approval_rejection_cap: default_approval_rejection_cap(),
        }
    }
}

fn default_oscillation() -> u32 {
    3
}
fn default_auto_loop() -> bool {
    true
}
fn default_global_timeout() -> u64 {
    3600
}
fn default_approval_rejection_cap() -> u32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDef {
    pub state_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_iter")]
    pub max_iterations: u32,
    #[serde(default)]
    #[serde(rename = "loop")]
    pub loop_state: bool,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub sub_states: Vec<SubStateDef>,
    #[serde(default)]
    pub hooks: Vec<crate::hooks::HookRef>,
}

fn default_true() -> bool {
    true
}
fn default_max_iter() -> u32 {
    3
}

impl StateDef {
    pub fn add_sub_state(&mut self, sub: SubStateDef, position: Option<usize>) {
        let pos = position.unwrap_or(self.sub_states.len());
        let idx = pos.min(self.sub_states.len());
        self.sub_states.insert(idx, sub);
    }

    pub fn remove_sub_state(&mut self, sub_id: &str) -> Option<SubStateDef> {
        self.sub_states
            .iter()
            .position(|s| s.id == sub_id)
            .map(|idx| self.sub_states.remove(idx))
    }

    pub fn move_sub_state(&mut self, from: usize, to: usize) -> bool {
        if from >= self.sub_states.len() || to >= self.sub_states.len() {
            return false;
        }
        let item = self.sub_states.remove(from);
        self.sub_states.insert(to, item);
        true
    }

    pub fn get_checklist_criteria(&self) -> Vec<String> {
        self.sub_states
            .iter()
            .find(|s| s.sub_state_type == SubStateType::Checklist)
            .and_then(|s| s.criteria.clone())
            .unwrap_or_default()
    }

    /// The index of this macro's first `enabled` sub-state of type
    /// `Execute` (SPEC_loopback.md: the loop-back target). Lives here, on
    /// the shared `StateDef` type, rather than duplicated in
    /// `protocol-fsm`/`protocol-manifest`, so the engine's loop-back
    /// target and the validator's Rule 9/10 checks can never drift: `fsm`
    /// depends on `manifest` (which calls `validate_profile` from
    /// `start_session`), so `manifest` cannot depend back on `fsm` to reuse
    /// a helper defined there -- `protocol-types` is the only crate both
    /// already depend on.
    pub fn first_enabled_execute_index(&self) -> Option<usize> {
        self.sub_states
            .iter()
            .position(|s| s.enabled && s.sub_state_type == SubStateType::Execute)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubStateDef {
    pub id: String,
    #[serde(rename = "type")]
    pub sub_state_type: SubStateType,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub criteria: Option<Vec<String>>,
    #[serde(default)]
    pub inject: Option<Injection>,
    #[serde(default)]
    pub verify: Option<Vec<VerifyCheck>>,
    /// Hex-encoded Ed25519 **public** key of the human approver
    /// (SPEC_human_approval.md). REQUIRED on a `human_approval` sub-state and
    /// forbidden on every other type — `validate_profile` Rule 11 hard-fails
    /// both directions, so a gate can never silently degrade into an
    /// unverifiable one (a missing key would otherwise leave nothing to check
    /// the signature against).
    #[serde(default)]
    pub approver_pubkey: Option<String>,
    /// Optional text shown to the human being asked to approve. Relayed to the
    /// agent inside the step payload next to the challenge, so it can be put in
    /// front of the person who holds the key.
    #[serde(default)]
    pub approval_prompt: Option<String>,
    #[serde(default)]
    pub hooks: Vec<crate::hooks::HookRef>,
}

/// A client-side (trusted driver) verification check bound to a checklist
/// criterion. The enforcer/fsm ignores this entirely (presence-only stays
/// unchanged); it is consumed by the orchestrator driver, which runs the
/// command and derives the criterion's evidence from the real result
/// instead of trusting whatever the LLM claims.
///
/// `criterion` must name one of the sub-state's declared `criteria`
/// (enforced by `validate_profile`, Rule 8) — otherwise the check would be a
/// silent no-op. If several checks target the same criterion they run in
/// order and the last one decides the criterion's fate (pass overwrites,
/// fail strips).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyCheck {
    /// The checklist criterion this check fills. Must be a member of the
    /// sub-state's `criteria`.
    pub criterion: String,
    /// Shell command run (via `sh -c`) in the driver's work dir.
    pub command: String,
    /// Exit code that means "pass". Default 0.
    #[serde(default)]
    pub expect_exit: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubStateType {
    Inject,
    Execute,
    Review,
    Checklist,
    /// A gate an AI agent cannot satisfy on its own: advancing past it requires
    /// an Ed25519 signature, produced by a human holding the profile's
    /// `approver_pubkey`, over a challenge the engine issued on entry
    /// (SPEC_human_approval.md). Unlike `checklist`, presence is NOT enough —
    /// this is the one sub-state type whose evidence the enforcer verifies
    /// cryptographically rather than merely observes.
    HumanApproval,
}

impl std::fmt::Display for SubStateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubStateType::Inject => write!(f, "inject"),
            SubStateType::Execute => write!(f, "execute"),
            SubStateType::Review => write!(f, "review"),
            SubStateType::Checklist => write!(f, "checklist"),
            SubStateType::HumanApproval => write!(f, "human_approval"),
        }
    }
}

impl Profile {
    #[deprecated(note = "use ProfileEditor")]
    pub fn add_sub_state(
        &mut self,
        state_id: &str,
        sub: SubStateDef,
        position: Option<usize>,
    ) -> bool {
        if let Some(state) = self.pipeline.iter_mut().find(|s| s.state_id == state_id) {
            state.add_sub_state(sub, position);
            true
        } else {
            false
        }
    }

    #[deprecated(note = "use ProfileEditor")]
    pub fn remove_sub_state(&mut self, state_id: &str, sub_id: &str) -> Option<SubStateDef> {
        self.pipeline
            .iter_mut()
            .find(|s| s.state_id == state_id)
            .and_then(|state| state.remove_sub_state(sub_id))
    }

    #[deprecated(note = "use ProfileEditor")]
    pub fn move_sub_state(&mut self, state_id: &str, from: usize, to: usize) -> bool {
        if let Some(state) = self.pipeline.iter_mut().find(|s| s.state_id == state_id) {
            state.move_sub_state(from, to)
        } else {
            false
        }
    }

    pub fn append_state(&mut self, state: StateDef) {
        self.pipeline.push(state);
    }

    pub fn insert_state(&mut self, state: StateDef, position: usize) {
        let idx = position.min(self.pipeline.len());
        self.pipeline.insert(idx, state);
    }

    pub fn enabled_states(&self) -> impl Iterator<Item = &StateDef> {
        self.pipeline.iter().filter(|s| s.enabled)
    }

    pub fn total_sub_states(&self) -> usize {
        self.pipeline.iter().map(|s| s.sub_states.len()).sum()
    }

    pub fn clone(&self, new_name: &str) -> Profile {
        let mut cloned = <Profile as Clone>::clone(self);
        cloned.name = new_name.to_string();
        cloned.protected = false;
        cloned.cloned_from = Some(self.name.clone());
        cloned.created_at = chrono::Utc::now().to_rfc3339();
        cloned
    }

    /// SPEC_config_layer.md "Macro-level `enabled` — normalize at load": a
    /// clone whose `pipeline` keeps only `enabled` macros, everything else
    /// identical. Pure (does not mutate `self`) and transient/in-memory
    /// only — callers that persist a `Profile` (e.g.
    /// `ProfileManager::save_profile`) must NEVER call this first, or a
    /// disabled macro would be silently stripped from disk. Apply this only
    /// where a `Profile` becomes a stepping/navigation structure
    /// (`ProfileFsmEngine::new`, the orchestrator driver's own copy), so
    /// `pipeline.first()`/`.last()`/`[idx+1]`/`len()-1` are already correct
    /// with zero further logic changes.
    pub fn with_enabled_macros_only(&self) -> Profile {
        let mut normalized = <Profile as Clone>::clone(self);
        normalized.pipeline.retain(|m| m.enabled);
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checklist_macro(state_id: &str, enabled: bool) -> StateDef {
        StateDef {
            state_id: state_id.to_string(),
            name: state_id.to_string(),
            description: String::new(),
            system_prompt: None,
            enabled,
            max_iterations: 3,
            loop_state: false,
            icon: None,
            sub_states: vec![SubStateDef {
                id: "checklist".to_string(),
                sub_state_type: SubStateType::Checklist,
                name: "Checklist".to_string(),
                description: String::new(),
                enabled: true,
                criteria: Some(vec!["done".to_string()]),
                inject: None,
                verify: None,
                approver_pubkey: None,
                approval_prompt: None,
                hooks: Vec::new(),
            }],
            hooks: Vec::new(),
        }
    }

    #[test]
    fn with_enabled_macros_only_keeps_only_enabled_macros_and_does_not_mutate_self() {
        let profile = Profile {
            name: "p".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            protected: false,
            created_at: String::new(),
            cloned_from: None,
            settings: ProfileSettings::default(),
            pipeline: vec![
                checklist_macro("a", true),
                checklist_macro("b", false),
                checklist_macro("c", true),
            ],
            output_contract: None,
        };

        let normalized = profile.with_enabled_macros_only();

        let ids: Vec<&str> = normalized
            .pipeline
            .iter()
            .map(|m| m.state_id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "c"]);

        // Pure: self is untouched.
        assert_eq!(profile.pipeline.len(), 3);
        let self_ids: Vec<&str> = profile
            .pipeline
            .iter()
            .map(|m| m.state_id.as_str())
            .collect();
        assert_eq!(self_ids, vec!["a", "b", "c"]);
    }
}

/// Declarative injection directive for an `inject`-type sub-state
/// (SPEC_v3 §1.1). All fields optional; a profile author fills in
/// whichever source(s) the sub-state should pull from.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Injection {
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub skill: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
}
