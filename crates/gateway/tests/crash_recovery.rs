// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Acceptance tests for crash-recovery wiring.
//!
//! "In-memory session gone" simulation: `LEDGER_DIR` (`./ledger`) is a
//! fixed path relative to the process cwd, so a real second-process restart
//! would still see the same on-disk ledger dir. These tests construct a
//! SECOND, independent `ProtocolServer` instance (its own empty in-memory
//! `sessions` map, just like a fresh process would have) sharing the same
//! scratch cwd/ledger dir as the first, and drive requests against the
//! second instance. This is a faithful simulation of a process restart: the
//! only thing a restart changes here is the in-memory map going from
//! populated to empty while the on-disk ledger + sidecar survive -- exactly
//! what constructing a second `ProtocolServer` reproduces, and it exercises
//! the real `recover_into_cache` code path (not a test-only shortcut).

use rmcp::handler::server::wrapper::Parameters;

use protocol_gateway::server::{
    ProtocolGetStateRequest, ProtocolServer, ProtocolStartRequest, ProtocolSubmitMilestoneRequest,
};

/// Two macros: `draft` (non-final: inject `s1`, checklist `chk1`) and `ship`
/// (final: checklist `chk2`). `with_hooks` optionally attaches an inline
/// hook to `s1` and macro `draft` purely to exercise acceptance (6)
/// (passivity) -- hooks are otherwise irrelevant to every other test here.
fn write_profile(dir: &std::path::Path, with_hooks: bool) -> String {
    let profile_path = dir.join("profile.yaml");
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
    let profile_yaml = format!(
        r#"
name: "crash-recovery-test"
version: "1.0.0"
description: "crash recovery acceptance test profile"

pipeline:
  - state_id: "draft"
    name: "Draft"
    max_iterations: 5
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
  - state_id: "ship"
    name: "Ship"
    max_iterations: 5
    sub_states:
      - id: "chk2"
        type: checklist
        name: "Ship checklist"
        criteria:
          - "shipped"
"#,
        hooks_block = hooks_block
    );
    std::fs::write(&profile_path, profile_yaml).unwrap();
    profile_path.to_string_lossy().into_owned()
}

/// Serializes cwd-scoped tests (`std::env::set_current_dir` is
/// process-global), mirroring `session_id_validation.rs`'s `ScratchCwd`.
static CWD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct ScratchCwd {
    original: std::path::PathBuf,
    tempdir: tempfile::TempDir,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl ScratchCwd {
    async fn enter() -> Self {
        let guard = CWD_LOCK.lock().await;
        let original = std::env::current_dir().unwrap();
        let tempdir = tempfile::TempDir::new().unwrap();
        std::env::set_current_dir(tempdir.path()).unwrap();
        Self {
            original,
            tempdir,
            _guard: guard,
        }
    }

    fn path(&self) -> &std::path::Path {
        self.tempdir.path()
    }
}

impl Drop for ScratchCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

/// Starts a session on `server`, advances past `s1` into `chk1` (the
/// recovered position both (1) and (2) assert against).
async fn start_and_advance_to_chk1(server: &ProtocolServer, manifest_path: &str, session_id: &str) {
    let start = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: manifest_path.to_string(),
            session_id: Some(session_id.to_string()),
            initial_context: None,
        }))
        .await
        .expect("protocol_start must succeed");
    assert_eq!(start.0["position"]["sub_state_id"], "s1");

    let advanced = server
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: session_id.to_string(),
            macro_id: "draft".to_string(),
            sub_state_id: "s1".to_string(),
            evidence: None,
            output: None,
        }))
        .await
        .expect("plain advance from s1 must succeed");
    assert_eq!(advanced.0["position"]["sub_state_id"], "chk1");
}

/// (1) + (4): round-trip recover mid-session, and the sidecar was written
/// correctly by `protocol_start` with no leftover `.tmp`.
#[tokio::test]
async fn recovers_position_after_in_memory_engine_is_gone() {
    let scratch = ScratchCwd::enter().await;
    let manifest_path = write_profile(scratch.path(), false);
    let session_id = "recover-1";

    let server_a = ProtocolServer::new();
    start_and_advance_to_chk1(&server_a, &manifest_path, session_id).await;
    drop(server_a); // simulate the process's in-memory map disappearing

    // (4) sidecar written atomically, correct content, no .tmp left.
    let sidecar_path = scratch
        .path()
        .join("ledger")
        .join(format!("{session_id}.manifest.json"));
    let sidecar: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sidecar_path).expect("sidecar must exist"))
            .expect("sidecar must be valid json");
    assert_eq!(sidecar["manifest_path"], manifest_path);
    assert_eq!(sidecar["version"], "1.0.0");
    assert!(
        !scratch
            .path()
            .join("ledger")
            .join(format!("{session_id}.manifest.json.tmp"))
            .exists(),
        "no .tmp sidecar file must remain after a successful start"
    );

    // (1) a SECOND server instance (empty in-memory map) recovers the exact
    // position/status via protocol_get_state, not SESSION_NOT_FOUND.
    let server_b = ProtocolServer::new();
    let state = server_b
        .protocol_get_state(Parameters(ProtocolGetStateRequest {
            session_id: session_id.to_string(),
        }))
        .await
        .expect("protocol_get_state must recover the session, not 404");

    assert_eq!(state.0["session_state"], "active");
    assert_eq!(state.0["position"]["macro_id"], "draft");
    assert_eq!(state.0["position"]["sub_state_id"], "chk1");
}

/// (2): after the drop, `protocol_submit_milestone` for the recovered
/// position advances correctly -- the rebuilt engine is fully live.
#[tokio::test]
async fn recovered_session_is_live_not_a_snapshot() {
    let scratch = ScratchCwd::enter().await;
    let manifest_path = write_profile(scratch.path(), false);
    let session_id = "recover-2";

    let server_a = ProtocolServer::new();
    start_and_advance_to_chk1(&server_a, &manifest_path, session_id).await;
    drop(server_a);

    let server_b = ProtocolServer::new();
    // Cache miss on submit_milestone directly (no prior get_state call),
    // proving the recovery path is wired into both tools independently.
    let advanced = server_b
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: session_id.to_string(),
            macro_id: "draft".to_string(),
            sub_state_id: "chk1".to_string(),
            evidence: Some(
                [("done".to_string(), serde_json::json!(true))]
                    .into_iter()
                    .collect(),
            ),
            output: None,
        }))
        .await
        .expect("submit_milestone on a recovered session must succeed");

    assert_eq!(advanced.0["advanced"], true);
    assert_eq!(advanced.0["position"]["macro_id"], "ship");
    assert_eq!(advanced.0["position"]["sub_state_id"], "chk2");

    // And the position genuinely persisted on server_b's own map now --
    // a second call against server_b for the OLD position must be rejected
    // as a step mismatch (not silently re-accepted), proving it is live.
    let mismatch = server_b
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: session_id.to_string(),
            macro_id: "draft".to_string(),
            sub_state_id: "chk1".to_string(),
            evidence: Some(
                [("done".to_string(), serde_json::json!(true))]
                    .into_iter()
                    .collect(),
            ),
            output: None,
        }))
        .await;
    let mismatch = match mismatch {
        Err(e) => e,
        Ok(_) => panic!("resubmitting a stale position must be rejected"),
    };
    assert!(mismatch.message.contains("Step mismatch") || mismatch.message.contains("mismatch"));
}

/// (3): a session_id with no sidecar/ledger anywhere still 404s -- recovery
/// never invents sessions.
#[tokio::test]
async fn unknown_session_still_404s() {
    let scratch = ScratchCwd::enter().await;
    let _manifest_path = write_profile(scratch.path(), false); // unused; just needs `./ledger` dir context

    let server = ProtocolServer::new();
    let result = server
        .protocol_get_state(Parameters(ProtocolGetStateRequest {
            session_id: "totally-unknown-session".to_string(),
        }))
        .await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("a session with no sidecar must 404, not be recovered"),
    };

    assert!(
        err.message.contains("Session not found") || err.message.contains("not found"),
        "expected a not-found error, got: {}",
        err.message
    );
}

/// (5): if the manifest referenced by the sidecar is removed before
/// recovery, the request fails loud with a load error -- never a silent or
/// partial serve.
#[tokio::test]
async fn stale_manifest_fails_loud_on_recover() {
    let scratch = ScratchCwd::enter().await;
    let manifest_path = write_profile(scratch.path(), false);
    let session_id = "recover-stale";

    let server_a = ProtocolServer::new();
    start_and_advance_to_chk1(&server_a, &manifest_path, session_id).await;
    drop(server_a);

    // Remove the manifest the sidecar points at.
    std::fs::remove_file(&manifest_path).unwrap();

    let server_b = ProtocolServer::new();
    let result = server_b
        .protocol_get_state(Parameters(ProtocolGetStateRequest {
            session_id: session_id.to_string(),
        }))
        .await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("recovery must fail loud when the manifest no longer loads"),
    };

    assert!(
        err.message.contains("Failed to load profile") || err.message.contains("load"),
        "expected a profile-load failure, got: {}",
        err.message
    );

    // And the session must NOT have been silently inserted into server_b's
    // in-memory map as some partial/stale state.
    let follow_up = server_b
        .protocol_get_state(Parameters(ProtocolGetStateRequest {
            session_id: session_id.to_string(),
        }))
        .await;
    assert!(
        follow_up.is_err(),
        "a failed recovery must not leave a partial session behind"
    );
}

/// (6) passivity: a hooks-bearing profile recovers identically to the same
/// profile with hooks stripped -- recovery never touches hooks.
#[tokio::test]
async fn passivity_hooks_bearing_profile_recovers_identically() {
    for with_hooks in [false, true] {
        let scratch = ScratchCwd::enter().await;
        let manifest_path = write_profile(scratch.path(), with_hooks);
        let session_id = "recover-passivity";

        let server_a = ProtocolServer::new();
        start_and_advance_to_chk1(&server_a, &manifest_path, session_id).await;
        drop(server_a);

        let server_b = ProtocolServer::new();
        let state = server_b
            .protocol_get_state(Parameters(ProtocolGetStateRequest {
                session_id: session_id.to_string(),
            }))
            .await
            .unwrap_or_else(|e| {
                panic!("recovery must succeed regardless of hooks (with_hooks={with_hooks}): {e}")
            });

        assert_eq!(state.0["session_state"], "active");
        assert_eq!(state.0["position"]["macro_id"], "draft");
        assert_eq!(state.0["position"]["sub_state_id"], "chk1");
        // scratch (and its ledger dir) drops here, so the next iteration's
        // fresh ScratchCwd starts from a clean slate.
    }
}
