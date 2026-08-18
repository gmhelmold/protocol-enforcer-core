// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Profile-lifecycle verbs for `ProfileEditor` — create / clone / delete /
//! rename. Split out of `editor/mod.rs` (which keeps the apply/preview core,
//! settings verbs, and the shared helpers) to stay under the file-size cap.
//! Additional inherent `impl ProfileEditor` block, same as the other `*_ops`
//! modules.

use std::fs;

use protocol_types::profile::Profile;

use super::{atomic_write, scaffold_profile, EditError, ProfileEditor};
use crate::profile_manager::is_safe_name;
use crate::profile_validator::validate_profile;

impl ProfileEditor {
    pub fn create(&self, name: &str, description: &str, force: bool) -> Result<Profile, EditError> {
        if !is_safe_name(name) {
            return Err(EditError::UnsafeName(name.to_string()));
        }
        if self.manager.profile_exists(name) {
            if !force {
                return Err(EditError::AlreadyExists(name.to_string()));
            }
            let existing = self.manager.load_profile(name).map_err(EditError::Io)?;
            if existing.protected {
                return Err(EditError::Protected(name.to_string()));
            }
        }
        let profile = scaffold_profile(name, description);
        validate_profile(&profile, &self.library).map_err(EditError::Validation)?;
        let path = self.path_for(name);
        let content = serde_yaml::to_string(&profile)
            .map_err(|e| EditError::Io(format!("serialize error: {e}")))?;
        atomic_write(&path, content.as_bytes())?;
        Ok(profile)
    }

    /// Clone `source` into a new profile `new_name`. `AlreadyExists` unless
    /// `force`. The source may itself be protected (cloning FROM a
    /// protected profile is how new work starts); the clone is never
    /// protected.
    pub fn clone(&self, source: &str, new_name: &str, force: bool) -> Result<Profile, EditError> {
        if !is_safe_name(new_name) {
            return Err(EditError::UnsafeName(new_name.to_string()));
        }
        let source_profile = self
            .manager
            .load_profile(source)
            .map_err(|_| EditError::NotFound(source.to_string()))?;
        if self.manager.profile_exists(new_name) {
            if !force {
                return Err(EditError::AlreadyExists(new_name.to_string()));
            }
            let existing = self.manager.load_profile(new_name).map_err(EditError::Io)?;
            if existing.protected {
                return Err(EditError::Protected(new_name.to_string()));
            }
        }
        let cloned = source_profile.clone(new_name);
        validate_profile(&cloned, &self.library).map_err(EditError::Validation)?;
        let path = self.path_for(new_name);
        let content = serde_yaml::to_string(&cloned)
            .map_err(|e| EditError::Io(format!("serialize error: {e}")))?;
        atomic_write(&path, content.as_bytes())?;
        Ok(cloned)
    }

    /// Delete profile `name`. Rejected with `Protected` for a protected
    /// profile, `NotFound` if it does not exist.
    pub fn delete(&self, name: &str) -> Result<(), EditError> {
        let (profile, path, _bytes) = self.load_with_bytes(name)?;
        if profile.protected {
            return Err(EditError::Protected(name.to_string()));
        }
        fs::remove_file(&path).map_err(|e| EditError::Io(e.to_string()))?;
        Ok(())
    }

    /// Rename profile `name` to `new_name` (a new file; the old file is
    /// removed once the new one is safely written). `AlreadyExists` unless
    /// `force` when `new_name` already has a profile on disk.
    pub fn rename(&self, name: &str, new_name: &str, force: bool) -> Result<Profile, EditError> {
        if !is_safe_name(new_name) {
            return Err(EditError::UnsafeName(new_name.to_string()));
        }
        let (mut profile, old_path, original_bytes) = self.load_with_bytes(name)?;
        if profile.protected {
            return Err(EditError::Protected(name.to_string()));
        }
        if name != new_name && self.manager.profile_exists(new_name) && !force {
            return Err(EditError::AlreadyExists(new_name.to_string()));
        }
        profile.name = new_name.to_string();
        validate_profile(&profile, &self.library).map_err(EditError::Validation)?;

        let current = fs::read(&old_path).map_err(|e| EditError::Io(e.to_string()))?;
        if current != original_bytes {
            return Err(EditError::Conflict);
        }

        let new_path = self.path_for(new_name);
        let content = serde_yaml::to_string(&profile)
            .map_err(|e| EditError::Io(format!("serialize error: {e}")))?;
        atomic_write(&new_path, content.as_bytes())?;
        if old_path != new_path {
            fs::remove_file(&old_path).map_err(|e| EditError::Io(e.to_string()))?;
        }
        Ok(profile)
    }
}
