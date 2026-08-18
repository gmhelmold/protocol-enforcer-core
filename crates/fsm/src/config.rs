// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! FSM configuration

use protocol_types::profile::ProfileSettings;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct FsmConfig {
    /// Wall-clock budget for a whole session; 0 disables the breaker. Read by
    /// `ProfileFsmEngine` as the `GlobalTimeout` circuit breaker.
    pub global_timeout_seconds: u64,
    /// Base directory under which the enforcer's `ArtifactStore` writes/reads
    /// offloaded blobs (`<root>/.artifacts/...`). Deployment infra (like the
    /// ledger dir), so the GATEWAY sets it from the environment after
    /// `from_settings`; it is NOT profile-configurable. Defaults to `.`.
    pub artifact_root: PathBuf,
    /// SHA-256 (lowercase hex) of the profile file bytes as loaded. The engine
    /// holds a parsed `Profile`, not the bytes, so the GATEWAY computes this and
    /// passes it through here; the engine only copies it into `SessionStarted`
    /// (SPEC_transcript_notary.md §5). `None` when the caller did not supply it.
    pub profile_sha256: Option<String>,
    /// Base directory the output-contract write is confined under
    /// (`validate_and_write_output`). A resolved destination that escapes this
    /// base is refused — the defense-in-depth half of security-audit FIX 1 (an
    /// absolute/`..` destination that slipped past the load-time validator must
    /// still be unable to steer the write outside the sandbox). There is no
    /// profile-configurable output base, so this defaults to the process cwd:
    /// the cwd IS the contract's base.
    pub output_base: PathBuf,
}

/// The process cwd, the default confinement base for the output contract.
/// Falls back to `.` if the cwd cannot be read (the write then resolves against
/// the current directory exactly as `std::fs` would).
fn default_output_base() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

impl FsmConfig {
    /// Seed the runtime config from a profile's `settings`. This is how the
    /// gateway makes `global_timeout_seconds` profile-configurable (it is NOT
    /// read from env). Other breaker knobs (`oscillation_detection_threshold`,
    /// `auto_loop_on_checklist_failure`) are read straight off the profile by
    /// the engine, so they are not mirrored here. `artifact_root` is deployment
    /// infra and defaults to `.`; the gateway overrides it from env.
    pub fn from_settings(settings: &ProfileSettings) -> Self {
        Self {
            global_timeout_seconds: settings.global_timeout_seconds,
            artifact_root: PathBuf::from("."),
            profile_sha256: None,
            output_base: default_output_base(),
        }
    }
}

impl Default for FsmConfig {
    fn default() -> Self {
        Self {
            global_timeout_seconds: 3600,
            artifact_root: PathBuf::from("."),
            profile_sha256: None,
            output_base: default_output_base(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_settings_mirrors_global_timeout() {
        let settings = ProfileSettings {
            global_timeout_seconds: 42,
            ..ProfileSettings::default()
        };
        assert_eq!(
            FsmConfig::from_settings(&settings).global_timeout_seconds,
            42
        );
    }

    #[test]
    fn from_settings_carries_zero_disabled_sentinel() {
        let settings = ProfileSettings {
            global_timeout_seconds: 0,
            ..ProfileSettings::default()
        };
        assert_eq!(
            FsmConfig::from_settings(&settings).global_timeout_seconds,
            0
        );
    }

    #[test]
    fn default_matches_settings_default() {
        assert_eq!(
            FsmConfig::default().global_timeout_seconds,
            ProfileSettings::default().global_timeout_seconds
        );
    }
}
