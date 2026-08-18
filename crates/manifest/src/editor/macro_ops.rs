// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Macro-level (`StateDef`) mutation verbs for `ProfileEditor`.

use protocol_types::profile::Profile;

use super::{find_macro, scaffold_macro, EditError, ProfileEditor};

impl ProfileEditor {
    /// Add a new macro, scaffolded as one enabled trailing checklist
    /// sub-state with a placeholder criterion (immediately VALID). Defaults
    /// to the end of the pipeline when `position` is `None`.
    pub fn add_macro(
        &self,
        name: &str,
        state_id: &str,
        macro_name: &str,
        position: Option<usize>,
    ) -> Result<Profile, EditError> {
        let state_id = state_id.to_string();
        let macro_name = macro_name.to_string();
        self.apply(name, move |p| {
            let state = scaffold_macro(&state_id, &macro_name);
            let pos = position.unwrap_or(p.pipeline.len()).min(p.pipeline.len());
            p.pipeline.insert(pos, state);
            Ok(())
        })
    }

    pub fn remove_macro(&self, name: &str, macro_id: &str) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        self.apply(name, move |p| {
            let idx = p
                .pipeline
                .iter()
                .position(|s| s.state_id == macro_id)
                .ok_or_else(|| {
                    EditError::Validation(vec![format!("macro '{macro_id}' not found")])
                })?;
            p.pipeline.remove(idx);
            Ok(())
        })
    }

    pub fn move_macro(&self, name: &str, macro_id: &str, to: usize) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        self.apply(name, move |p| {
            let from = p
                .pipeline
                .iter()
                .position(|s| s.state_id == macro_id)
                .ok_or_else(|| {
                    EditError::Validation(vec![format!("macro '{macro_id}' not found")])
                })?;
            let to = to.min(p.pipeline.len().saturating_sub(1));
            let item = p.pipeline.remove(from);
            p.pipeline.insert(to, item);
            Ok(())
        })
    }

    pub fn set_macro_name(
        &self,
        name: &str,
        macro_id: &str,
        new_name: &str,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let new_name = new_name.to_string();
        self.apply(name, move |p| {
            find_macro(p, &macro_id)?.name = new_name;
            Ok(())
        })
    }

    pub fn set_macro_description(
        &self,
        name: &str,
        macro_id: &str,
        description: &str,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let description = description.to_string();
        self.apply(name, move |p| {
            find_macro(p, &macro_id)?.description = description;
            Ok(())
        })
    }

    pub fn set_macro_max_iterations(
        &self,
        name: &str,
        macro_id: &str,
        max_iterations: u32,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        self.apply(name, move |p| {
            find_macro(p, &macro_id)?.max_iterations = max_iterations;
            Ok(())
        })
    }

    /// Set the macro's `loop` (`loop_state`) flag, which gates macro loop-back.
    pub fn set_macro_loop(
        &self,
        name: &str,
        macro_id: &str,
        loop_state: bool,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        self.apply(name, move |p| {
            find_macro(p, &macro_id)?.loop_state = loop_state;
            Ok(())
        })
    }

    pub fn enable_macro(&self, name: &str, macro_id: &str) -> Result<Profile, EditError> {
        self.set_macro_enabled(name, macro_id, true)
    }

    pub fn disable_macro(&self, name: &str, macro_id: &str) -> Result<Profile, EditError> {
        self.set_macro_enabled(name, macro_id, false)
    }

    fn set_macro_enabled(
        &self,
        name: &str,
        macro_id: &str,
        enabled: bool,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        self.apply(name, move |p| {
            find_macro(p, &macro_id)?.enabled = enabled;
            Ok(())
        })
    }
}
