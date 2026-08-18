// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! `ProfileEditor` — the editing core (SPEC_config_layer.md WP-A).
//!
//! A single typed mutation API that is the ONLY path allowed to mutate a
//! profile. The CLI (WP-A2, not implemented here) and any future
//! programmatic builder are thin skins over this. Every mutation method
//! follows the same contract: load (via the hardened, name-sanitized
//! `ProfileManager::load_profile` path) -> apply the in-memory mutation ->
//! `validate_profile` (STRICT) -> on `Ok`, atomic-save; on `Err`, return the
//! violations and touch nothing on disk.
//!
//! Split across submodules to stay under the workspace's 600-line-per-file
//! cap: this module holds the core (`edit`/`apply`/`preview`, atomic save +
//! compare-and-swap, and the profile-level verbs); `macro_ops`, `sub_ops`,
//! `injection_ops`, and `hook_ops` hold the macro / sub-state / injection /
//! hook verbs as additional `impl ProfileEditor` blocks over the same
//! struct.

mod hook_ops;
mod injection_ops;
mod lifecycle_ops;
mod macro_ops;
mod sub_ops;

use std::fs;
use std::path::{Path, PathBuf};

use protocol_library::Library;
use protocol_types::profile::{Profile, ProfileSettings};

use crate::profile_manager::is_safe_name;
use crate::profile_validator::validate_profile;
use crate::ProfileManager;

/// Every error a `ProfileEditor` mutation can return.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// No profile file exists on disk for the given name.
    #[error("profile '{0}' not found")]
    NotFound(String),
    /// The profile (on disk, or the mutation's target) is `protected` and
    /// cannot be modified/deleted/overwritten.
    #[error("profile '{0}' is protected and cannot be modified")]
    Protected(String),
    /// A supplied name would traverse outside the profiles directory (or is
    /// otherwise empty/invalid) — mirrors `ProfileManager`'s hardened guard.
    #[error("invalid profile name: '{0}'")]
    UnsafeName(String),
    /// `create`/`clone`/`rename` targeted a name that already has a profile
    /// on disk, without `force: true`. Closes the hole where
    /// `ProfileManager::save_profile` only blocked the *protected* case and
    /// silently clobbered an unprotected same-name profile.
    #[error("profile '{0}' already exists (pass force=true to overwrite)")]
    AlreadyExists(String),
    /// The resulting profile failed the STRICT `validate_profile` (or a
    /// dedicated pre-check, e.g. an unresolved skill/protocol, or an
    /// unknown macro/sub-state id). Nothing was written.
    #[error("validation failed: {0:?}")]
    Validation(Vec<String>),
    /// The on-disk file changed between this editor's load and its save
    /// (compare-and-swap failure) — a concurrent edit landed first. Nothing
    /// was written; the caller should reload and retry.
    #[error("profile changed on disk since it was loaded (concurrent edit) — reload and retry")]
    Conflict,
    /// Filesystem I/O failure.
    #[error("I/O error: {0}")]
    Io(String),
}

/// Wraps a `ProfileManager` (profiles dir) + a `Library` (skill/protocol
/// resolution) and is the sole mutation path for profiles.
pub struct ProfileEditor {
    pub(crate) manager: ProfileManager,
    pub(crate) library: Library,
}

impl ProfileEditor {
    pub fn new(profiles_dir: impl AsRef<Path>, library: Library) -> Self {
        Self {
            manager: ProfileManager::new(profiles_dir),
            library,
        }
    }

    /// Wrap an existing `ProfileManager` instead of constructing a fresh one
    /// (e.g. when a caller already has one rooted at the right dir).
    pub fn with_manager(manager: ProfileManager, library: Library) -> Self {
        Self { manager, library }
    }

    pub fn manager(&self) -> &ProfileManager {
        &self.manager
    }

    pub(crate) fn path_for(&self, name: &str) -> PathBuf {
        self.manager.profiles_dir().join(format!("{}.yaml", name))
    }

    /// Load a profile plus the raw bytes on disk at load time (the
    /// compare-and-swap baseline) and its path.
    pub(crate) fn load_with_bytes(
        &self,
        name: &str,
    ) -> Result<(Profile, PathBuf, Vec<u8>), EditError> {
        if !is_safe_name(name) {
            return Err(EditError::UnsafeName(name.to_string()));
        }
        let path = self.path_for(name);
        if !path.exists() {
            return Err(EditError::NotFound(name.to_string()));
        }
        let bytes = fs::read(&path).map_err(|e| EditError::Io(e.to_string()))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let profile: Profile =
            serde_yaml::from_str(&text).map_err(|e| EditError::Io(format!("parse error: {e}")))?;
        Ok((profile, path, bytes))
    }

    /// Atomically write `profile` to `path`, first re-reading `path` and
    /// rejecting with `Conflict` if its bytes no longer match
    /// `original_bytes` (compare-and-swap against a lost update). The write
    /// itself goes to a temp file in the same directory, then an atomic
    /// rename, so a crash mid-write never truncates the target.
    fn atomic_save_cas(
        &self,
        path: &Path,
        original_bytes: &[u8],
        profile: &Profile,
    ) -> Result<(), EditError> {
        let current = fs::read(path).map_err(|e| EditError::Io(e.to_string()))?;
        if current != original_bytes {
            return Err(EditError::Conflict);
        }
        let content = serde_yaml::to_string(profile)
            .map_err(|e| EditError::Io(format!("serialize error: {e}")))?;
        atomic_write(path, content.as_bytes())
    }

    /// The core mutation primitive every public verb funnels through: load
    /// -> apply `mutate` -> STRICT-validate -> (if `persist`) atomic
    /// compare-and-swap save. Returns the resulting in-memory `Profile`
    /// either way, so a non-persisting call is a full preview (result +
    /// validation, nothing written).
    fn edit<F>(&self, name: &str, persist: bool, mutate: F) -> Result<Profile, EditError>
    where
        F: FnOnce(&mut Profile) -> Result<(), EditError>,
    {
        let (mut profile, path, original_bytes) = self.load_with_bytes(name)?;
        // `protected` only blocks WRITES: a `preview` (persist = false) may
        // still load/mutate-in-memory/validate a protected profile (e.g. a
        // future CLI `--dry-run` inspecting "what would this do?"), it just
        // can never reach `apply`'s atomic save.
        if persist && profile.protected {
            return Err(EditError::Protected(name.to_string()));
        }
        mutate(&mut profile)?;
        validate_profile(&profile, &self.library).map_err(EditError::Validation)?;
        if persist {
            self.atomic_save_cas(&path, &original_bytes, &profile)?;
        }
        Ok(profile)
    }

    /// Apply a mutation and persist it (the "real" edit path).
    pub(crate) fn apply<F>(&self, name: &str, mutate: F) -> Result<Profile, EditError>
    where
        F: FnOnce(&mut Profile) -> Result<(), EditError>,
    {
        self.edit(name, true, mutate)
    }

    /// Load + mutate + validate WITHOUT persisting — the dry-run primitive a
    /// future CLI builds `--dry-run` on top of, by reusing the exact same
    /// mutate closures the `apply_*` verbs use.
    pub fn preview<F>(&self, name: &str, mutate: F) -> Result<Profile, EditError>
    where
        F: FnOnce(&mut Profile) -> Result<(), EditError>,
    {
        self.edit(name, false, mutate)
    }

    // -- Profile-level verbs -------------------------------------------

    /// Create a new, immediately-valid, empty-scaffold profile named `name`.
    /// Rejected with `AlreadyExists` if a profile already exists at that
    /// name, unless `force` is set (in which case an existing *protected*
    /// profile still cannot be overwritten — `Protected` wins).
    pub fn set_description(&self, name: &str, description: &str) -> Result<Profile, EditError> {
        let description = description.to_string();
        self.apply(name, move |p| {
            p.description = description;
            Ok(())
        })
    }

    /// Set the profile-wide oscillation-detection threshold, the
    /// auto-loop-on-checklist-failure master switch, and/or the global session
    /// timeout (seconds; `0` disables). `None` leaves the corresponding field
    /// unchanged.
    pub fn set_settings(
        &self,
        name: &str,
        oscillation_detection_threshold: Option<u32>,
        auto_loop_on_checklist_failure: Option<bool>,
        global_timeout_seconds: Option<u64>,
    ) -> Result<Profile, EditError> {
        self.apply(name, move |p| {
            apply_settings(
                &mut p.settings,
                oscillation_detection_threshold,
                auto_loop_on_checklist_failure,
                global_timeout_seconds,
            );
            Ok(())
        })
    }
}

fn apply_settings(
    settings: &mut ProfileSettings,
    oscillation_detection_threshold: Option<u32>,
    auto_loop_on_checklist_failure: Option<bool>,
    global_timeout_seconds: Option<u64>,
) {
    if let Some(threshold) = oscillation_detection_threshold {
        settings.oscillation_detection_threshold = threshold;
    }
    if let Some(auto_loop) = auto_loop_on_checklist_failure {
        settings.auto_loop_on_checklist_failure = auto_loop;
    }
    if let Some(timeout) = global_timeout_seconds {
        settings.global_timeout_seconds = timeout;
    }
}

/// Write `content` to `path` via a temp file in the same directory + an
/// atomic rename, so a crash mid-write never leaves a truncated profile.
pub(crate) fn atomic_write(path: &Path, content: &[u8]) -> Result<(), EditError> {
    let dir = path
        .parent()
        .ok_or_else(|| EditError::Io("profile path has no parent directory".to_string()))?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| EditError::Io("profile path has no file name".to_string()))?;
    let tmp_path = dir.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::write(&tmp_path, content).map_err(|e| EditError::Io(e.to_string()))?;
    fs::rename(&tmp_path, path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        EditError::Io(e.to_string())
    })?;
    Ok(())
}

/// An immediately-VALID empty profile scaffold: one enabled macro with a
/// single enabled trailing checklist sub-state carrying a placeholder
/// criterion.
pub(crate) fn scaffold_profile(name: &str, description: &str) -> Profile {
    use protocol_types::profile::{StateDef, SubStateDef, SubStateType};
    Profile {
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
            description: String::new(),
            system_prompt: None,
            enabled: true,
            max_iterations: 3,
            loop_state: false,
            icon: None,
            sub_states: vec![SubStateDef {
                id: "checklist".to_string(),
                sub_state_type: SubStateType::Checklist,
                name: "Checklist".to_string(),
                description: String::new(),
                enabled: true,
                criteria: Some(vec!["criterion_1".to_string()]),
                inject: None,
                verify: None,
                approver_pubkey: None,
                approval_prompt: None,
                hooks: Vec::new(),
            }],
            hooks: Vec::new(),
        }],
        output_contract: None,
    }
}

/// Build a scaffold `StateDef` for a new macro: one enabled trailing
/// checklist sub-state with a placeholder criterion, so `add_macro` always
/// produces an immediately-valid macro.
pub(crate) fn scaffold_macro(state_id: &str, name: &str) -> protocol_types::profile::StateDef {
    use protocol_types::profile::{StateDef, SubStateDef, SubStateType};
    StateDef {
        state_id: state_id.to_string(),
        name: name.to_string(),
        description: String::new(),
        system_prompt: None,
        enabled: true,
        max_iterations: 3,
        loop_state: false,
        icon: None,
        sub_states: vec![SubStateDef {
            id: "checklist".to_string(),
            sub_state_type: SubStateType::Checklist,
            name: "Checklist".to_string(),
            description: String::new(),
            enabled: true,
            criteria: Some(vec!["criterion_1".to_string()]),
            inject: None,
            verify: None,
            approver_pubkey: None,
            approval_prompt: None,
            hooks: Vec::new(),
        }],
        hooks: Vec::new(),
    }
}

/// Build a scaffold `SubStateDef` of the given type: a checklist gets a
/// non-empty placeholder criteria list (so the macro stays valid
/// immediately); an execute/inject/review sub-state gets no criteria.
pub(crate) fn scaffold_sub(
    sub_id: &str,
    sub_type: protocol_types::profile::SubStateType,
    name: &str,
) -> protocol_types::profile::SubStateDef {
    use protocol_types::profile::{SubStateDef, SubStateType};
    let criteria = if sub_type == SubStateType::Checklist {
        Some(vec!["criterion_1".to_string()])
    } else {
        None
    };
    SubStateDef {
        id: sub_id.to_string(),
        sub_state_type: sub_type,
        name: name.to_string(),
        description: String::new(),
        enabled: true,
        criteria,
        inject: None,
        verify: None,
        approver_pubkey: None,
        approval_prompt: None,
        hooks: Vec::new(),
    }
}

/// Find a macro (`StateDef`) by id, or a `Validation` error naming it.
pub(crate) fn find_macro<'a>(
    profile: &'a mut Profile,
    macro_id: &str,
) -> Result<&'a mut protocol_types::profile::StateDef, EditError> {
    profile
        .pipeline
        .iter_mut()
        .find(|s| s.state_id == macro_id)
        .ok_or_else(|| EditError::Validation(vec![format!("macro '{macro_id}' not found")]))
}

/// Find a sub-state by id within `macro_id`, or a `Validation` error naming
/// it.
pub(crate) fn find_sub<'a>(
    profile: &'a mut Profile,
    macro_id: &str,
    sub_id: &str,
) -> Result<&'a mut protocol_types::profile::SubStateDef, EditError> {
    let state = find_macro(profile, macro_id)?;
    state
        .sub_states
        .iter_mut()
        .find(|s| s.id == sub_id)
        .ok_or_else(|| {
            EditError::Validation(vec![format!(
                "sub-state '{sub_id}' not found in macro '{macro_id}'"
            )])
        })
}

#[cfg(test)]
mod tests {
    //! Internals-only tests that need the private `apply`/`edit` primitive
    //! (e.g. to simulate a concurrent write mid-mutation for the CAS test) --
    //! the public-API acceptance suite lives in
    //! `tests/profile_editor_tests.rs`.
    use super::*;
    use protocol_types::profile::{ProfileSettings, StateDef, SubStateDef, SubStateType};
    use tempfile::TempDir;

    fn checklist_profile(name: &str) -> Profile {
        Profile {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: "fixture".to_string(),
            protected: false,
            created_at: String::new(),
            cloned_from: None,
            settings: ProfileSettings::default(),
            pipeline: vec![StateDef {
                state_id: "phase1".to_string(),
                name: "Phase 1".to_string(),
                description: String::new(),
                system_prompt: None,
                enabled: true,
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
            }],
            output_contract: None,
        }
    }

    fn empty_library(dir: &TempDir) -> Library {
        Library::new(dir.path())
    }

    /// SPEC_config_layer.md `EditError::Conflict`: if the on-disk file
    /// changes between this editor's load and its save (simulated here by
    /// tampering with the file from inside the mutate closure itself --
    /// standing in for "a second editor landed a write concurrently"), the
    /// save is rejected and the concurrent write is left untouched (no lost
    /// update in either direction).
    #[test]
    fn apply_rejects_with_conflict_when_file_changes_mid_edit() {
        let dir = TempDir::new().unwrap();
        let manager = ProfileManager::new(dir.path());
        manager.save_profile(&checklist_profile("cas")).unwrap();
        let path = dir.path().join("cas.yaml");

        let editor =
            ProfileEditor::with_manager(ProfileManager::new(dir.path()), empty_library(&dir));

        let tamper_path = path.clone();
        let result = editor.apply("cas", move |p| {
            // Simulate a concurrent editor landing a write in between our
            // load and our save.
            std::fs::write(
                &tamper_path,
                "name: \"cas\"\nversion: \"1.0.0\"\ndescription: \"tampered\"\npipeline: []\n",
            )
            .unwrap();
            p.description = "our edit".to_string();
            Ok(())
        });

        assert!(
            matches!(result, Err(EditError::Conflict)),
            "expected Conflict, got {result:?}"
        );

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains("tampered"),
            "the concurrent writer's content must survive untouched: {on_disk}"
        );
    }

    #[test]
    fn preview_does_not_persist() {
        let dir = TempDir::new().unwrap();
        let manager = ProfileManager::new(dir.path());
        manager.save_profile(&checklist_profile("prev")).unwrap();
        let path = dir.path().join("prev.yaml");
        let before = std::fs::read_to_string(&path).unwrap();

        let editor =
            ProfileEditor::with_manager(ProfileManager::new(dir.path()), empty_library(&dir));
        let result = editor.preview("prev", |p| {
            p.description = "would change".to_string();
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(result.unwrap().description, "would change");

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(before, after, "preview must never write to disk");
    }
}
