// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Hook (`HookRef`) attach/detach verbs for `ProfileEditor`
//! (SPEC_plugins.md WP-4) -- mirrors `injection_ops.rs`'s shape.
//!
//! Every verb here goes through the same validate-on-write + atomic-write +
//! CAS path as the rest of `ProfileEditor`, but ADDITIONALLY runs
//! `hook_validator::validate_hooks` before persisting (H1-H6), so attaching
//! a malformed or unresolvable hook is rejected and nothing is written.
//! `validate_hooks` itself is a separate function from the strict
//! served-path `validate_profile` (passivity invariant, C1) -- this module
//! is one of its two callers (the other is the driver, WP-3).

use protocol_types::hooks::HookRef;
use protocol_types::profile::Profile;

use super::{find_macro, find_sub, EditError, ProfileEditor};
use crate::hook_validator::validate_hooks;
use crate::profile_validator::validate_profile;

impl ProfileEditor {
    /// Load -> mutate -> STRICT-validate -> `validate_hooks` -> atomic CAS
    /// save. Like `ProfileEditor::apply`, but additionally hook-validates
    /// before persisting.
    fn edit_with_hook_validation<F>(&self, name: &str, mutate: F) -> Result<Profile, EditError>
    where
        F: FnOnce(&mut Profile) -> Result<(), EditError>,
    {
        let (mut profile, path, original_bytes) = self.load_with_bytes(name)?;
        if profile.protected {
            return Err(EditError::Protected(name.to_string()));
        }
        mutate(&mut profile)?;
        validate_profile(&profile, &self.library).map_err(EditError::Validation)?;
        validate_hooks(&profile, &self.library).map_err(EditError::Validation)?;
        self.atomic_save_cas(&path, &original_bytes, &profile)?;
        Ok(profile)
    }

    pub fn attach_macro_hook(
        &self,
        name: &str,
        macro_id: &str,
        hook_ref: HookRef,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        self.edit_with_hook_validation(name, move |p| {
            find_macro(p, &macro_id)?.hooks.push(hook_ref);
            Ok(())
        })
    }

    pub fn detach_macro_hook(
        &self,
        name: &str,
        macro_id: &str,
        index: usize,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        self.edit_with_hook_validation(name, move |p| {
            let state = find_macro(p, &macro_id)?;
            if index >= state.hooks.len() {
                return Err(EditError::Validation(vec![format!(
                    "macro '{macro_id}' has no hook at index {index}"
                )]));
            }
            state.hooks.remove(index);
            Ok(())
        })
    }

    pub fn attach_sub_hook(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        hook_ref: HookRef,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.edit_with_hook_validation(name, move |p| {
            find_sub(p, &macro_id, &sub_id)?.hooks.push(hook_ref);
            Ok(())
        })
    }

    pub fn detach_sub_hook(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        index: usize,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.edit_with_hook_validation(name, move |p| {
            let sub = find_sub(p, &macro_id, &sub_id)?;
            if index >= sub.hooks.len() {
                return Err(EditError::Validation(vec![format!(
                    "sub-state '{sub_id}' has no hook at index {index}"
                )]));
            }
            sub.hooks.remove(index);
            Ok(())
        })
    }
}
