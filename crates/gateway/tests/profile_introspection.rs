// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Acceptance tests for read-only profile introspection over MCP.
//!
//! `protocol_profile_list`/`protocol_profile_show` resolve their profiles
//! dir from `PROTOCOL_PROFILES_DIR` (see `protocol_gateway::introspect`).
//! `std::env::set_var` is process-global, so these tests are serialized
//! through `ENV_LOCK` (mirroring `crash_recovery.rs`'s `CWD_LOCK` pattern
//! for `std::env::set_current_dir`).

use rmcp::handler::server::wrapper::Parameters;

use protocol_gateway::server::{ProtocolProfileShowRequest, ProtocolServer};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct ScratchProfilesDir {
    _tempdir: tempfile::TempDir,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl ScratchProfilesDir {
    async fn enter() -> Self {
        let guard = ENV_LOCK.lock().await;
        let tempdir = tempfile::TempDir::new().unwrap();
        std::env::set_var("PROTOCOL_PROFILES_DIR", tempdir.path());
        Self {
            _tempdir: tempdir,
            _guard: guard,
        }
    }

    fn path(&self) -> &std::path::Path {
        self._tempdir.path()
    }
}

impl Drop for ScratchProfilesDir {
    fn drop(&mut self) {
        std::env::remove_var("PROTOCOL_PROFILES_DIR");
    }
}

/// `with_hooks` optionally attaches an inline hook to the `inject` sub-state
/// and its macro, to exercise acceptance (3) hook-blindness; irrelevant to
/// every other test.
fn alpha_profile_yaml(with_hooks: bool) -> String {
    let hooks_block = if with_hooks {
        r#"
        hooks:
          - inline:
              id: "audit-log"
              version: "1.0.0"
              kind: validate
              events: ["pre-substate-enter"]
              command: "true""#
    } else {
        ""
    };
    format!(
        r#"
name: "alpha"
version: "2.3.1"
description: "the alpha test profile"

pipeline:
  - state_id: "draft"
    name: "Draft"
    sub_states:
      - id: "s1"
        type: inject
        name: "Step 1"
        inject:
          prompt: "do step 1"{hooks_block}
      - id: "chk1"
        type: checklist
        name: "Draft checklist"
        criteria:
          - "done"
          - "reviewed"
  - state_id: "ship"
    name: "Ship"
    sub_states:
      - id: "chk2"
        type: checklist
        name: "Ship checklist"
        criteria:
          - "shipped"
"#,
        hooks_block = hooks_block
    )
}

fn beta_profile_yaml() -> &'static str {
    r#"
name: "beta"
version: "1.0.0"
description: "the beta test profile"

pipeline:
  - state_id: "only"
    name: "Only"
    sub_states:
      - id: "chk"
        type: checklist
        name: "Checklist"
        criteria:
          - "x"
"#
}

/// Snapshot of a directory: sorted (filename, bytes) pairs, used to assert
/// byte-for-byte disk stability (acceptance 6).
fn snapshot_dir(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut entries: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(e.path()).unwrap();
            (name, bytes)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// (1) `protocol_profile_list` returns exactly the profiles present, nothing
/// extra.
#[tokio::test]
async fn list_returns_exactly_the_profiles_present() {
    let scratch = ScratchProfilesDir::enter().await;
    std::fs::write(scratch.path().join("alpha.yaml"), alpha_profile_yaml(false)).unwrap();
    std::fs::write(scratch.path().join("beta.yaml"), beta_profile_yaml()).unwrap();

    let server = ProtocolServer::new();
    let result = server.protocol_profile_list().await.unwrap();

    let mut names: Vec<String> = result.0["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
}

/// (2) `protocol_profile_show` returns the correct structure: macro
/// ids/names in order, sub ids/types, checklist criteria present.
#[tokio::test]
async fn show_returns_correct_structure() {
    let scratch = ScratchProfilesDir::enter().await;
    std::fs::write(scratch.path().join("alpha.yaml"), alpha_profile_yaml(false)).unwrap();

    let server = ProtocolServer::new();
    let result = server
        .protocol_profile_show(Parameters(ProtocolProfileShowRequest {
            name: "alpha".to_string(),
        }))
        .await
        .unwrap();

    assert_eq!(result.0["name"], "alpha");
    assert_eq!(result.0["version"], "2.3.1");
    assert_eq!(result.0["description"], "the alpha test profile");

    let pipeline = result.0["pipeline"].as_array().unwrap();
    assert_eq!(pipeline.len(), 2);

    assert_eq!(pipeline[0]["state_id"], "draft");
    assert_eq!(pipeline[0]["name"], "Draft");
    assert_eq!(pipeline[0]["enabled"], true);
    assert_eq!(pipeline[1]["state_id"], "ship");
    assert_eq!(pipeline[1]["name"], "Ship");

    let draft_subs = pipeline[0]["sub_states"].as_array().unwrap();
    assert_eq!(draft_subs.len(), 2);
    assert_eq!(draft_subs[0]["id"], "s1");
    assert_eq!(draft_subs[0]["type"], "inject");
    assert_eq!(draft_subs[1]["id"], "chk1");
    assert_eq!(draft_subs[1]["type"], "checklist");
    assert_eq!(
        draft_subs[1]["criteria"],
        serde_json::json!(["done", "reviewed"])
    );

    let ship_subs = pipeline[1]["sub_states"].as_array().unwrap();
    assert_eq!(ship_subs[0]["id"], "chk2");
    assert_eq!(ship_subs[0]["criteria"], serde_json::json!(["shipped"]));
}

/// (3) Hook-blindness: a profile WITH inline hooks and the same profile with
/// hooks stripped produce byte-identical `protocol_profile_show` output; the
/// serialized value has no "hooks" key anywhere.
#[tokio::test]
async fn show_is_hook_blind() {
    let mut outputs = Vec::new();
    for with_hooks in [false, true] {
        let scratch = ScratchProfilesDir::enter().await;
        std::fs::write(
            scratch.path().join("alpha.yaml"),
            alpha_profile_yaml(with_hooks),
        )
        .unwrap();

        let server = ProtocolServer::new();
        let result = server
            .protocol_profile_show(Parameters(ProtocolProfileShowRequest {
                name: "alpha".to_string(),
            }))
            .await
            .unwrap();
        outputs.push(result.0);
    }

    assert_eq!(
        outputs[0], outputs[1],
        "show output must be identical regardless of hooks"
    );

    let serialized = serde_json::to_string(&outputs[1]).unwrap();
    assert!(
        !serialized.contains("hooks"),
        "served surface must never emit a hooks field, got: {}",
        serialized
    );
}

/// (4) `protocol_profile_show` on a non-existent name -> a clean error, not
/// a panic.
#[tokio::test]
async fn show_on_missing_profile_is_a_clean_error() {
    let scratch = ScratchProfilesDir::enter().await;
    std::fs::write(scratch.path().join("alpha.yaml"), alpha_profile_yaml(false)).unwrap();

    let server = ProtocolServer::new();
    let result = server
        .protocol_profile_show(Parameters(ProtocolProfileShowRequest {
            name: "does-not-exist".to_string(),
        }))
        .await;

    assert!(result.is_err(), "missing profile must be Err, not Ok");
}

/// (5) A `../`-style name is rejected by the safe-name guard; no filesystem
/// escape occurs (nothing outside the profiles dir is read/created).
#[tokio::test]
async fn show_rejects_path_traversal_name() {
    let scratch = ScratchProfilesDir::enter().await;
    std::fs::write(scratch.path().join("alpha.yaml"), alpha_profile_yaml(false)).unwrap();

    // A real secret file that would be readable from the profiles dir's
    // parent via a naive `../` join, to prove the guard actually stops it.
    let parent = scratch.path().parent().unwrap();
    let secret_name = format!(
        "introspection-secret-{}.yaml",
        scratch.path().file_name().unwrap().to_string_lossy()
    );
    std::fs::write(parent.join(&secret_name), "name: leaked").unwrap();

    let server = ProtocolServer::new();
    let result = server
        .protocol_profile_show(Parameters(ProtocolProfileShowRequest {
            name: format!("../{}", secret_name.trim_end_matches(".yaml")),
        }))
        .await;

    assert!(result.is_err(), "a '../'-style name must be rejected");

    std::fs::remove_file(parent.join(&secret_name)).ok();
}

/// (6) Neither tool mutates disk: the profiles dir is byte-identical before
/// and after calling both tools.
#[tokio::test]
async fn tools_never_mutate_the_profiles_dir() {
    let scratch = ScratchProfilesDir::enter().await;
    std::fs::write(scratch.path().join("alpha.yaml"), alpha_profile_yaml(false)).unwrap();
    std::fs::write(scratch.path().join("beta.yaml"), beta_profile_yaml()).unwrap();

    let before = snapshot_dir(scratch.path());

    let server = ProtocolServer::new();
    let _ = server.protocol_profile_list().await.unwrap();
    let _ = server
        .protocol_profile_show(Parameters(ProtocolProfileShowRequest {
            name: "alpha".to_string(),
        }))
        .await
        .unwrap();
    // Also exercise the error paths -- they must not mutate disk either.
    let _ = server
        .protocol_profile_show(Parameters(ProtocolProfileShowRequest {
            name: "missing".to_string(),
        }))
        .await;
    let _ = server
        .protocol_profile_show(Parameters(ProtocolProfileShowRequest {
            name: "../escape".to_string(),
        }))
        .await;

    let after = snapshot_dir(scratch.path());
    assert_eq!(before, after, "profiles dir must be byte-identical");
}
