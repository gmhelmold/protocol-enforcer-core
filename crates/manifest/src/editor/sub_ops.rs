// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Sub-state-level (`SubStateDef`) mutation verbs for `ProfileEditor`.

use protocol_types::profile::{Profile, SubStateType, VerifyCheck};

use super::{find_macro, find_sub, scaffold_sub, EditError, ProfileEditor};

impl ProfileEditor {
    /// Add a sub-state of `sub_type` to macro `macro_id`, scaffolded valid:
    /// a checklist gets a non-empty placeholder criteria list, anything else
    /// gets none. Smart default position: a checklist defaults to the end
    /// (becoming the new trailing checklist); anything else defaults to
    /// just before the macro's existing trailing checklist (if any), so the
    /// "checklist must be last" rule is never broken by an obvious `add`.
    pub fn add_sub(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        sub_type: SubStateType,
        sub_name: &str,
        position: Option<usize>,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        let sub_name = sub_name.to_string();
        self.apply(name, move |p| {
            let state = find_macro(p, &macro_id)?;
            let default_pos = if sub_type == SubStateType::Checklist {
                state.sub_states.len()
            } else {
                state
                    .sub_states
                    .iter()
                    .position(|s| s.sub_state_type == SubStateType::Checklist)
                    .unwrap_or(state.sub_states.len())
            };
            let pos = position.unwrap_or(default_pos).min(state.sub_states.len());
            state
                .sub_states
                .insert(pos, scaffold_sub(&sub_id, sub_type, &sub_name));
            Ok(())
        })
    }

    pub fn remove_sub(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.apply(name, move |p| {
            let state = find_macro(p, &macro_id)?;
            let idx = state
                .sub_states
                .iter()
                .position(|s| s.id == sub_id)
                .ok_or_else(|| {
                    EditError::Validation(vec![format!(
                        "sub-state '{sub_id}' not found in macro '{macro_id}'"
                    )])
                })?;
            state.sub_states.remove(idx);
            Ok(())
        })
    }

    pub fn move_sub(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        to: usize,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.apply(name, move |p| {
            let state = find_macro(p, &macro_id)?;
            let from = state
                .sub_states
                .iter()
                .position(|s| s.id == sub_id)
                .ok_or_else(|| {
                    EditError::Validation(vec![format!(
                        "sub-state '{sub_id}' not found in macro '{macro_id}'"
                    )])
                })?;
            let to = to.min(state.sub_states.len().saturating_sub(1));
            let item = state.sub_states.remove(from);
            state.sub_states.insert(to, item);
            Ok(())
        })
    }

    pub fn set_sub_name(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        new_name: &str,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        let new_name = new_name.to_string();
        self.apply(name, move |p| {
            find_sub(p, &macro_id, &sub_id)?.name = new_name;
            Ok(())
        })
    }

    pub fn set_sub_description(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        description: &str,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        let description = description.to_string();
        self.apply(name, move |p| {
            find_sub(p, &macro_id, &sub_id)?.description = description;
            Ok(())
        })
    }

    pub fn enable_sub(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
    ) -> Result<Profile, EditError> {
        self.set_sub_enabled(name, macro_id, sub_id, true)
    }

    pub fn disable_sub(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
    ) -> Result<Profile, EditError> {
        self.set_sub_enabled(name, macro_id, sub_id, false)
    }

    fn set_sub_enabled(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        enabled: bool,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.apply(name, move |p| {
            find_sub(p, &macro_id, &sub_id)?.enabled = enabled;
            Ok(())
        })
    }

    /// Set a checklist sub-state's criteria list. Rejected pre-validation if
    /// the target sub-state is not a `Checklist` (a clearer message than
    /// letting the generic Rule-4 post-validation fire).
    pub fn set_criteria(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        criteria: Vec<String>,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.apply(name, move |p| {
            let sub = find_sub(p, &macro_id, &sub_id)?;
            if sub.sub_state_type != SubStateType::Checklist {
                return Err(EditError::Validation(vec![format!(
                    "sub-state '{sub_id}' is not a checklist; only checklist sub-states have criteria"
                )]));
            }
            sub.criteria = Some(criteria);
            Ok(())
        })
    }

    /// Set a sub-state's `verify` checks (each must target a declared
    /// criterion — enforced by the generic post-validation, Rule 8).
    pub fn set_verify(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        verify: Vec<VerifyCheck>,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.apply(name, move |p| {
            let sub = find_sub(p, &macro_id, &sub_id)?;
            sub.verify = if verify.is_empty() {
                None
            } else {
                Some(verify)
            };
            Ok(())
        })
    }
}
