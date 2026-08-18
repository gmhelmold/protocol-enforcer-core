// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Skill/protocol library resolver.
//!
//! Resolves reusable skill/protocol content by name from a directory tree:
//! `<root>/skills/<name>.md`, `<root>/protocols/<name>.md`.

use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Which sub-directory of the library root to resolve a name against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibKind {
    Skill,
    Protocol,
    Hook,
}

impl LibKind {
    fn dir_name(self) -> &'static str {
        match self {
            LibKind::Skill => "skills",
            LibKind::Protocol => "protocols",
            LibKind::Hook => "hooks",
        }
    }
}

impl fmt::Display for LibKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LibKind::Skill => write!(f, "skill"),
            LibKind::Protocol => write!(f, "protocol"),
            LibKind::Hook => write!(f, "hook"),
        }
    }
}

/// Resolves skill/protocol content from a filesystem directory tree.
#[derive(Debug, Clone)]
pub struct Library {
    root: PathBuf,
}

/// Errors produced while resolving library content.
#[derive(Debug, thiserror::Error)]
pub enum LibError {
    #[error("library entry not found: {0}")]
    NotFound(String),
    #[error("library IO error: {0}")]
    Io(String),
    #[error("library entry '{0}' failed to parse: {1}")]
    Parse(String, String),
}

const DEFAULT_LIBRARY_DIR: &str = "./library";
const LIBRARY_DIR_ENV_VAR: &str = "PROTOCOL_LIBRARY_DIR";

impl Library {
    /// Build a `Library` rooted at the given directory.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Build a `Library` rooted at `PROTOCOL_LIBRARY_DIR`, defaulting to `./library`.
    pub fn from_env() -> Self {
        let root =
            env::var(LIBRARY_DIR_ENV_VAR).unwrap_or_else(|_| DEFAULT_LIBRARY_DIR.to_string());
        Self::new(root)
    }

    /// Resolve `name` under `kind`'s sub-directory and return its text content.
    ///
    /// `name` must not contain path separators or `..` components — such
    /// names are rejected as `LibError::NotFound` rather than allowed to
    /// escape the library root.
    pub fn resolve(&self, kind: LibKind, name: &str) -> Result<String, LibError> {
        if !is_safe_name(name) {
            return Err(LibError::NotFound(name.to_string()));
        }

        let path = self.root.join(kind.dir_name()).join(format!("{name}.md"));

        match fs::read_to_string(&path) {
            Ok(content) => Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(LibError::NotFound(name.to_string()))
            }
            Err(e) => Err(LibError::Io(e.to_string())),
        }
    }

    /// Resolve `id` under the `hooks/` sub-directory and parse it as a
    /// structured `HookDef` (`hooks/<id>.yaml`, falling back to
    /// `hooks/<id>.yml`). Unlike `resolve` (raw text for skills/protocols),
    /// hook library entries are structured YAML, so this parses rather than
    /// returning raw content.
    pub fn resolve_hook(&self, id: &str) -> Result<protocol_types::hooks::HookDef, LibError> {
        if !is_safe_name(id) {
            return Err(LibError::NotFound(id.to_string()));
        }

        let dir = self.root.join(LibKind::Hook.dir_name());
        let yaml_path = dir.join(format!("{id}.yaml"));
        let yml_path = dir.join(format!("{id}.yml"));

        let path = if yaml_path.exists() {
            yaml_path
        } else {
            yml_path
        };

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(LibError::NotFound(id.to_string()));
            }
            Err(e) => return Err(LibError::Io(e.to_string())),
        };

        serde_yaml::from_str(&content).map_err(|e| LibError::Parse(id.to_string(), e.to_string()))
    }

    /// List every resolvable name under `kind`'s sub-directory (file stems
    /// of the `.md` entries), sorted. Used by the CLI's "did you mean?"
    /// suggestions (SPEC_config_layer.md zero-friction UX) — a read-only
    /// enumeration, never on the served/validation path. An unreadable or
    /// missing directory yields an empty list rather than an error.
    pub fn list_names(&self, kind: LibKind) -> Vec<String> {
        let dir = self.root.join(kind.dir_name());
        let mut names: Vec<String> = match fs::read_dir(&dir) {
            Ok(entries) => entries
                .flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
                .filter_map(|e| {
                    e.path()
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }

    /// List every resolvable hook id under `hooks/` (file stems of the
    /// `.yaml`/`.yml` entries), sorted. Mirrors `list_names` but for the
    /// structured hook library, whose entries carry a different extension.
    pub fn list_hook_names(&self) -> Vec<String> {
        let dir = self.root.join(LibKind::Hook.dir_name());
        let mut names: Vec<String> = match fs::read_dir(&dir) {
            Ok(entries) => entries
                .flatten()
                .filter(|e| {
                    matches!(
                        e.path().extension().and_then(|x| x.to_str()),
                        Some("yaml") | Some("yml")
                    )
                })
                .filter_map(|e| {
                    e.path()
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names.dedup();
        names
    }
}

/// Reject names that could traverse outside the library root.
fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && !name.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library")
    }

    #[test]
    fn resolve_skill_hit() {
        let lib = Library::new(fixture_root());
        let content = lib.resolve(LibKind::Skill, "tdd").expect("should resolve");
        assert!(content.contains("TDD"));
    }

    #[test]
    fn resolve_protocol_hit() {
        let lib = Library::new(fixture_root());
        let content = lib
            .resolve(LibKind::Protocol, "code_review")
            .expect("should resolve");
        assert!(content.contains("Code Review"));
    }

    #[test]
    fn list_names_lists_md_stems() {
        let lib = Library::new(fixture_root());
        assert_eq!(lib.list_names(LibKind::Skill), vec!["tdd".to_string()]);
        assert_eq!(
            lib.list_names(LibKind::Protocol),
            vec!["code_review".to_string()]
        );
    }

    #[test]
    fn resolve_hook_round_trips() {
        use protocol_types::hooks::{HookDef, HookEvent, HookKind};
        use std::collections::BTreeMap;

        let dir = tempfile::TempDir::new().expect("tempdir");
        let hooks_dir = dir.path().join("hooks");
        std::fs::create_dir_all(&hooks_dir).expect("mkdir hooks");

        let def = HookDef {
            id: "foo".to_string(),
            version: "1.0.0".to_string(),
            kind: HookKind::Validate,
            events: vec![HookEvent::PreSubstateEnter],
            command: "check.sh".to_string(),
            timeout_ms: 5_000,
            output_cap_bytes: 4096,
            fail_open: Some(false),
            inputs: BTreeMap::new(),
            verify_criteria: Vec::new(),
        };
        let yaml = serde_yaml::to_string(&def).expect("serialize HookDef");
        std::fs::write(hooks_dir.join("foo.yaml"), yaml).expect("write foo.yaml");

        let lib = Library::new(dir.path());
        let resolved = lib.resolve_hook("foo").expect("should resolve hook");

        assert_eq!(resolved.id, def.id);
        assert_eq!(resolved.version, def.version);
        assert_eq!(resolved.kind, def.kind);
        assert_eq!(resolved.events, def.events);
        assert_eq!(resolved.command, def.command);
        assert_eq!(resolved.timeout_ms, def.timeout_ms);
        assert_eq!(resolved.output_cap_bytes, def.output_cap_bytes);
        assert_eq!(resolved.fail_open, def.fail_open);
    }

    #[test]
    fn resolve_hook_missing_is_not_found() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let lib = Library::new(dir.path());
        let err = lib
            .resolve_hook("does_not_exist")
            .expect_err("should not resolve");
        assert!(matches!(err, LibError::NotFound(_)));
    }

    #[test]
    fn list_names_missing_dir_is_empty() {
        let lib = Library::new(PathBuf::from("/does/not/exist"));
        assert!(lib.list_names(LibKind::Skill).is_empty());
    }

    #[test]
    fn resolve_miss_returns_not_found() {
        let lib = Library::new(fixture_root());
        let err = lib
            .resolve(LibKind::Skill, "does_not_exist")
            .expect_err("should not resolve");
        match err {
            LibError::NotFound(name) => assert_eq!(name, "does_not_exist"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn resolve_rejects_path_traversal() {
        let lib = Library::new(fixture_root());

        for malicious in [
            "../secrets",
            "..",
            "a/../../etc/passwd",
            "foo/bar",
            "foo\\bar",
        ] {
            let err = lib
                .resolve(LibKind::Skill, malicious)
                .expect_err("traversal attempt must be rejected");
            assert!(
                matches!(err, LibError::NotFound(_)),
                "expected NotFound for {malicious}, got {err:?}"
            );
        }
    }

    #[test]
    fn from_env_reads_the_env_var() {
        // Both the override and default cases are asserted in ONE test so they
        // run sequentially: separate #[test] fns would race on this shared
        // process-wide env var under the default parallel test runner.
        std::env::set_var(LIBRARY_DIR_ENV_VAR, "/tmp/some-custom-library-dir");
        assert_eq!(
            Library::from_env().root,
            PathBuf::from("/tmp/some-custom-library-dir")
        );
        std::env::remove_var(LIBRARY_DIR_ENV_VAR);
        assert_eq!(Library::from_env().root, PathBuf::from(DEFAULT_LIBRARY_DIR));
    }
}
