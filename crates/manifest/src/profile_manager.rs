// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Profile system - data-driven state machine configuration
//!
//! WP0: the data shapes (`Profile`, `ProfileSettings`, `StateDef`,
//! `SubStateDef`, `SubStateType`, `Injection`) now live in
//! `protocol_types::profile` as the shared type foundation. This module
//! re-exports them and keeps the filesystem CRUD (`ProfileManager`).

use std::path::{Path, PathBuf};

pub use protocol_types::profile::*;

/// Reject names that could traverse outside the profiles directory.
///
/// Mirrors `crates/library/src/lib.rs`'s `is_safe_name`: any profile name
/// that becomes part of a filesystem path must be checked with this before
/// use, so `../../etc/passwd`-style names can never escape `profiles_dir`.
///
/// `pub` so `crate::editor` (WP-A `ProfileEditor`) and the gateway's
/// name-vs-path resolution (`protocol_start`) reuse this exact hardened guard
/// instead of re-implementing (and risking drift on) name sanitization.
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && !name.contains("..")
}

/// Profile manager - CRUD operations
pub struct ProfileManager {
    profiles_dir: PathBuf,
}

impl ProfileManager {
    pub fn new(profiles_dir: impl AsRef<Path>) -> Self {
        Self {
            profiles_dir: profiles_dir.as_ref().to_path_buf(),
        }
    }

    /// The profiles directory this manager is rooted at. Used by
    /// `crate::editor::ProfileEditor` to derive on-disk paths for the
    /// compare-and-swap / atomic-save machinery without duplicating the
    /// `<name>.yaml` naming convention.
    pub(crate) fn profiles_dir(&self) -> &Path {
        &self.profiles_dir
    }

    /// Whether a profile file already exists on disk for `name`. Unsafe
    /// names are reported as non-existent (never traverse to check).
    pub(crate) fn profile_exists(&self, name: &str) -> bool {
        is_safe_name(name) && self.profiles_dir.join(format!("{}.yaml", name)).exists()
    }

    pub fn list_profiles(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.profiles_dir) {
            for entry in entries.flatten() {
                if let Some(ext) = entry.path().extension() {
                    if ext == "yaml" || ext == "yml" {
                        if let Some(stem) = entry.path().file_stem() {
                            names.push(stem.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
        names.sort();
        names
    }

    pub fn load_profile(&self, name: &str) -> Result<Profile, String> {
        if !is_safe_name(name) {
            return Err(format!("Invalid profile name: '{}'", name));
        }
        let path = self.profiles_dir.join(format!("{}.yaml", name));
        if !path.exists() {
            return Err(format!("Profile '{}' not found at {:?}", name, path));
        }
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("Failed to read profile: {}", e))?;
        serde_yaml::from_str(&content).map_err(|e| format!("Failed to parse profile: {}", e))
    }

    /// Returns `Err` if a profile file already exists at the on-disk path for
    /// `name` and that ON-DISK profile is `protected`, regardless of what the
    /// in-memory `Profile` being written claims. This is what stops
    /// `clone_profile`/`create_empty_profile` (which always construct an
    /// in-memory profile with `protected = false`) from clobbering a
    /// protected profile file such as `profiles/default.yaml`.
    fn check_target_not_protected(&self, name: &str) -> Result<(), String> {
        let path = self.profiles_dir.join(format!("{}.yaml", name));
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read existing profile: {}", e))?;
        let existing: Profile = serde_yaml::from_str(&content)
            .map_err(|e| format!("Failed to parse existing profile: {}", e))?;
        if existing.protected {
            return Err(format!(
                "Profile '{}' is protected on disk and cannot be overwritten",
                name
            ));
        }
        Ok(())
    }

    pub fn save_profile(&self, profile: &Profile) -> Result<(), String> {
        if !is_safe_name(&profile.name) {
            return Err(format!("Invalid profile name: '{}'", profile.name));
        }
        if profile.protected {
            return Err(format!(
                "Profile '{}' is protected and cannot be modified",
                profile.name
            ));
        }
        self.check_target_not_protected(&profile.name)?;
        let path = self.profiles_dir.join(format!("{}.yaml", profile.name));
        let content = serde_yaml::to_string(profile)
            .map_err(|e| format!("Failed to serialize profile: {}", e))?;
        std::fs::write(&path, content).map_err(|e| format!("Failed to write profile: {}", e))?;
        Ok(())
    }

    pub fn clone_profile(&self, source_name: &str, new_name: &str) -> Result<Profile, String> {
        if !is_safe_name(source_name) {
            return Err(format!("Invalid profile name: '{}'", source_name));
        }
        if !is_safe_name(new_name) {
            return Err(format!("Invalid profile name: '{}'", new_name));
        }
        let mut profile = self.load_profile(source_name)?;
        profile.name = new_name.to_string();
        profile.protected = false;
        profile.cloned_from = Some(source_name.to_string());
        profile.created_at = chrono::Utc::now().to_rfc3339();
        self.save_profile(&profile)?;
        Ok(profile)
    }

    pub fn create_empty_profile(&self, name: &str, description: &str) -> Result<Profile, String> {
        if !is_safe_name(name) {
            return Err(format!("Invalid profile name: '{}'", name));
        }
        let profile = Profile {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: description.to_string(),
            protected: false,
            created_at: chrono::Utc::now().to_rfc3339(),
            cloned_from: None,
            settings: ProfileSettings::default(),
            pipeline: vec![StateDef {
                state_id: "phase1".to_string(),
                name: "Phase 1".to_string(),
                description: "Customize this phase".to_string(),
                system_prompt: None,
                enabled: true,
                max_iterations: 3,
                loop_state: false,
                icon: Some("1️⃣".to_string()),
                sub_states: vec![
                    SubStateDef {
                        id: "inject".to_string(),
                        sub_state_type: SubStateType::Inject,
                        name: "Inject Context".to_string(),
                        description: "Inject relevant context".to_string(),
                        enabled: true,
                        criteria: None,
                        inject: None,
                        verify: None,
                        approver_pubkey: None,
                        approval_prompt: None,
                        hooks: Vec::new(),
                    },
                    SubStateDef {
                        id: "execute".to_string(),
                        sub_state_type: SubStateType::Execute,
                        name: "Execute".to_string(),
                        description: "Execute the work".to_string(),
                        enabled: true,
                        criteria: None,
                        inject: None,
                        verify: None,
                        approver_pubkey: None,
                        approval_prompt: None,
                        hooks: Vec::new(),
                    },
                    SubStateDef {
                        id: "checklist".to_string(),
                        sub_state_type: SubStateType::Checklist,
                        name: "Checklist Gate".to_string(),
                        description: "Validate completion".to_string(),
                        enabled: true,
                        criteria: Some(vec!["item_1".to_string()]),
                        inject: None,
                        verify: None,
                        approver_pubkey: None,
                        approval_prompt: None,
                        hooks: Vec::new(),
                    },
                ],
                hooks: Vec::new(),
            }],
            output_contract: None,
        };
        self.save_profile(&profile)?;
        Ok(profile)
    }

    pub fn delete_profile(&self, name: &str) -> Result<(), String> {
        if !is_safe_name(name) {
            return Err(format!("Invalid profile name: '{}'", name));
        }
        let profile = self.load_profile(name)?;
        if profile.protected {
            return Err(format!(
                "Profile '{}' is protected and cannot be deleted",
                name
            ));
        }
        let path = self.profiles_dir.join(format!("{}.yaml", name));
        std::fs::remove_file(&path).map_err(|e| format!("Failed to delete profile: {}", e))?;
        Ok(())
    }

    pub fn validate_profile(&self, profile: &Profile) -> Result<Vec<String>, Vec<String>> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        if profile.name.is_empty() {
            errors.push("Profile name cannot be empty".to_string());
        }
        if profile.pipeline.is_empty() {
            errors.push("Pipeline must have at least one state".to_string());
        }

        let last_macro_id = profile.pipeline.last().map(|m| m.state_id.as_str());
        for (si, state) in profile.pipeline.iter().enumerate() {
            if state.state_id.is_empty() {
                errors.push(format!("State {} has empty state_id", si));
            }
            if state.sub_states.is_empty() {
                warnings.push(format!("State '{}' has no sub_states", state.state_id));
            }
            let has_checklist = state
                .sub_states
                .iter()
                .any(|s| s.sub_state_type == SubStateType::Checklist);
            if !has_checklist {
                warnings.push(format!(
                    "State '{}' has no checklist sub-state",
                    state.state_id
                ));
            }

            // SPEC_loopback.md Rules 9/10, surfaced as authoring warnings
            // here (the strict, hard-fail versions live in
            // `protocol_manifest::validate_profile`, run at `protocol_start`).
            if state.loop_state {
                if state.first_enabled_execute_index().is_none() {
                    warnings.push(format!(
                        "State '{}' has loop: true but no enabled execute sub-state \
                         (the loop-back target) -- protocol_start will reject this",
                        state.state_id
                    ));
                }
                if last_macro_id == Some(state.state_id.as_str()) {
                    warnings.push(format!(
                        "State '{}' has loop: true on the final pipeline macro \
                         (forbidden -- protocol_start will reject this)",
                        state.state_id
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(warnings)
        } else {
            Err(errors)
        }
    }

    #[deprecated(note = "use ProfileEditor")]
    pub fn add_state(
        &self,
        profile_name: &str,
        state: StateDef,
        position: Option<usize>,
    ) -> Result<(), String> {
        let mut profile = self.load_profile(profile_name)?;
        if profile.protected {
            return Err("Cannot modify protected profile".to_string());
        }
        let pos = position.unwrap_or(profile.pipeline.len());
        let idx = pos.min(profile.pipeline.len());
        profile.pipeline.insert(idx, state);
        self.save_profile(&profile)?;
        Ok(())
    }

    #[deprecated(note = "use ProfileEditor")]
    pub fn remove_state(&self, profile_name: &str, state_id: &str) -> Result<StateDef, String> {
        let mut profile = self.load_profile(profile_name)?;
        if profile.protected {
            return Err("Cannot modify protected profile".to_string());
        }
        if let Some(idx) = profile.pipeline.iter().position(|s| s.state_id == state_id) {
            let removed = profile.pipeline.remove(idx);
            self.save_profile(&profile)?;
            Ok(removed)
        } else {
            Err(format!("State '{}' not found", state_id))
        }
    }

    #[deprecated(note = "use ProfileEditor")]
    pub fn move_state(&self, profile_name: &str, from: usize, to: usize) -> Result<(), String> {
        let mut profile = self.load_profile(profile_name)?;
        if profile.protected {
            return Err("Cannot modify protected profile".to_string());
        }
        if from >= profile.pipeline.len() || to >= profile.pipeline.len() {
            return Err("Position out of bounds".to_string());
        }
        let item = profile.pipeline.remove(from);
        profile.pipeline.insert(to, item);
        self.save_profile(&profile)?;
        Ok(())
    }
}
