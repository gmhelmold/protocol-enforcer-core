// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Injection (`Injection`) mutation verbs for `ProfileEditor`.
//!
//! `attach_skill`/`attach_protocol` resolve against the `Library` directly
//! (a fast, dedicated "unresolved skill/protocol 'x'" message) IN ADDITION
//! to the generic post-mutation Rule-5 validation that would otherwise also
//! catch it — giving a clearer, more specific error before the mutation is
//! even applied.

use protocol_library::LibKind;
use protocol_types::profile::{Injection, Profile};

use super::{find_sub, EditError, ProfileEditor};

impl ProfileEditor {
    pub fn set_prompt(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        prompt: Option<String>,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.apply(name, move |p| {
            find_sub(p, &macro_id, &sub_id)?
                .inject
                .get_or_insert_with(Injection::default)
                .prompt = prompt;
            Ok(())
        })
    }

    pub fn set_context(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        context: Option<String>,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.apply(name, move |p| {
            find_sub(p, &macro_id, &sub_id)?
                .inject
                .get_or_insert_with(Injection::default)
                .context = context;
            Ok(())
        })
    }

    pub fn attach_skill(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        skill: &str,
    ) -> Result<Profile, EditError> {
        if self.library.resolve(LibKind::Skill, skill).is_err() {
            return Err(EditError::Validation(vec![format!(
                "unresolved skill '{skill}'"
            )]));
        }
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        let skill = skill.to_string();
        self.apply(name, move |p| {
            find_sub(p, &macro_id, &sub_id)?
                .inject
                .get_or_insert_with(Injection::default)
                .skill = Some(skill);
            Ok(())
        })
    }

    pub fn detach_skill(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.apply(name, move |p| {
            if let Some(inject) = find_sub(p, &macro_id, &sub_id)?.inject.as_mut() {
                inject.skill = None;
            }
            Ok(())
        })
    }

    pub fn attach_protocol(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
        protocol: &str,
    ) -> Result<Profile, EditError> {
        if self.library.resolve(LibKind::Protocol, protocol).is_err() {
            return Err(EditError::Validation(vec![format!(
                "unresolved protocol '{protocol}'"
            )]));
        }
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        let protocol = protocol.to_string();
        self.apply(name, move |p| {
            find_sub(p, &macro_id, &sub_id)?
                .inject
                .get_or_insert_with(Injection::default)
                .protocol = Some(protocol);
            Ok(())
        })
    }

    pub fn detach_protocol(
        &self,
        name: &str,
        macro_id: &str,
        sub_id: &str,
    ) -> Result<Profile, EditError> {
        let macro_id = macro_id.to_string();
        let sub_id = sub_id.to_string();
        self.apply(name, move |p| {
            if let Some(inject) = find_sub(p, &macro_id, &sub_id)?.inject.as_mut() {
                inject.protocol = None;
            }
            Ok(())
        })
    }
}
