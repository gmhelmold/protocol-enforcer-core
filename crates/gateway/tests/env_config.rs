// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Acceptance tests for the gateway's env-configurable paths.
//!
//! Two behaviors are covered:
//!   1. `PROTOCOL_LEDGER_DIR` redirects where per-session ledgers and
//!      crash-recovery sidecars are written (CHANGE A).
//!   2. `PROTOCOL_PROFILES_DIR`, when set, confines `protocol_start`'s
//!      `manifest_path` to that tree (opt-in confinement, CHANGE B).
//!
//! `std::env::set_var` is process-global, so these tests are serialized
//! through `ENV_LOCK` (mirroring `profile_introspection.rs`).

use rmcp::handler::server::wrapper::Parameters;

use protocol_gateway::server::{
    ProtocolServer, ProtocolStartRequest, ProtocolStoreArtifactRequest,
    ProtocolSubmitMilestoneRequest,
};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A minimal valid profile (an `inject` sub followed by the required terminal
/// `checklist` sub), matching the shape used across the other gateway tests.
fn minimal_profile_yaml() -> &'static str {
    r#"
name: "env-config-test"
version: "1.0.0"
description: "gateway env-config test profile"

pipeline:
  - state_id: "phase1"
    name: "Phase One"
    sub_states:
      - id: "intro"
        type: inject
        name: "Intro"
      - id: "gate"
        type: checklist
        name: "Gate"
        criteria:
          - "a"
"#
}

/// Holds the `ENV_LOCK` for the test's duration and unconditionally removes
/// both env vars this suite touches on drop (`remove_var` of an unset var is
/// harmless), so no test leaks a var into the next.
struct ScratchEnv {
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl ScratchEnv {
    async fn enter() -> Self {
        let guard = ENV_LOCK.lock().await;
        Self { _guard: guard }
    }
}

impl Drop for ScratchEnv {
    fn drop(&mut self) {
        std::env::remove_var("PROTOCOL_LEDGER_DIR");
        std::env::remove_var("PROTOCOL_PROFILES_DIR");
        std::env::remove_var("PROTOCOL_ARTIFACT_DIR");
        std::env::remove_var("PROTOCOL_MANIFEST_STRICT");
    }
}

/// `PROTOCOL_LEDGER_DIR` redirects both the per-session ledger and the
/// crash-recovery sidecar to the configured (absolute) directory.
#[tokio::test]
async fn ledger_dir_env_var_redirects_ledger_and_sidecar() {
    let _env = ScratchEnv::enter().await;
    let tempdir = tempfile::TempDir::new().unwrap();
    let ledgers = tempdir.path().join("ledgers");
    std::env::set_var("PROTOCOL_LEDGER_DIR", &ledgers);

    let manifest_path = tempdir.path().join("profile.yaml");
    std::fs::write(&manifest_path, minimal_profile_yaml()).unwrap();

    let server = ProtocolServer::new();
    let result = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: manifest_path.to_string_lossy().into_owned(),
            session_id: Some("sess-ledgerdir".to_string()),
            initial_context: None,
        }))
        .await
        .expect("protocol_start must succeed");

    assert_eq!(result.0["session_id"], "sess-ledgerdir");

    assert!(
        ledgers.join("sess-ledgerdir.jsonl").exists(),
        "ledger must be written under PROTOCOL_LEDGER_DIR"
    );
    assert!(
        ledgers.join("sess-ledgerdir.manifest.json").exists(),
        "sidecar must be written under PROTOCOL_LEDGER_DIR"
    );
    assert!(
        !std::path::Path::new("./ledger/sess-ledgerdir.jsonl").exists(),
        "ledger must not land in the default ./ledger dir"
    );
}

/// With `PROTOCOL_PROFILES_DIR` set, a `manifest_path` inside that tree is
/// accepted while an otherwise-valid one outside it is rejected.
#[tokio::test]
async fn manifest_path_outside_profiles_dir_is_rejected_when_confined() {
    let _env = ScratchEnv::enter().await;
    let tempdir = tempfile::TempDir::new().unwrap();
    let profiles = tempdir.path().join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();

    std::env::set_var("PROTOCOL_PROFILES_DIR", &profiles);
    std::env::set_var("PROTOCOL_LEDGER_DIR", tempdir.path().join("ledgers"));

    let inside = profiles.join("inside.yaml");
    std::fs::write(&inside, minimal_profile_yaml()).unwrap();
    let outside = tempdir.path().join("outside.yaml");
    std::fs::write(&outside, minimal_profile_yaml()).unwrap();

    let server = ProtocolServer::new();

    let ok = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: inside.to_string_lossy().into_owned(),
            session_id: Some("sess-inside".to_string()),
            initial_context: None,
        }))
        .await;
    assert!(
        ok.is_ok(),
        "a manifest_path inside PROTOCOL_PROFILES_DIR must be accepted, got: {:?}",
        ok.err()
    );

    let rejected = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: outside.to_string_lossy().into_owned(),
            session_id: Some("sess-outside".to_string()),
            initial_context: None,
        }))
        .await;
    assert!(
        rejected.is_err(),
        "a manifest_path outside PROTOCOL_PROFILES_DIR must be rejected"
    );
}

/// A bare profile NAME (no extension) resolves under `PROTOCOL_PROFILES_DIR`
/// to `<name>.yaml` and starts; a name with no matching file is a clean load
/// error. This is the ergonomic, confined-by-construction alternative to
/// passing a filesystem path.
#[tokio::test]
async fn bare_name_resolves_under_profiles_dir() {
    let _env = ScratchEnv::enter().await;
    let tempdir = tempfile::TempDir::new().unwrap();
    let profiles = tempdir.path().join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::env::set_var("PROTOCOL_PROFILES_DIR", &profiles);
    std::env::set_var("PROTOCOL_LEDGER_DIR", tempdir.path().join("ledgers"));

    std::fs::write(profiles.join("named.yaml"), minimal_profile_yaml()).unwrap();

    let server = ProtocolServer::new();

    // Bare name -> resolves to profiles/named.yaml and starts.
    let ok = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: "named".to_string(),
            session_id: Some("sess-name".to_string()),
            initial_context: None,
        }))
        .await
        .expect("a bare profile name must resolve and start");
    assert_eq!(ok.0["session_id"], "sess-name");
    assert_eq!(ok.0["position"]["sub_state_id"], "intro");

    // A name with no matching file is a clean error, not a panic.
    let missing = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: "no-such-profile".to_string(),
            session_id: Some("sess-missing".to_string()),
            initial_context: None,
        }))
        .await;
    assert!(missing.is_err(), "an unknown profile name must be rejected");
}

/// Confinement follows symlinks: a symlink INSIDE the confined tree that points
/// OUTSIDE it is rejected, because the path is canonicalized before the
/// `starts_with` check (and the canonicalized real path is what gets loaded —
/// closing the check-vs-load TOCTOU window).
#[cfg(unix)]
#[tokio::test]
async fn symlink_escaping_confinement_is_rejected() {
    let _env = ScratchEnv::enter().await;
    let tempdir = tempfile::TempDir::new().unwrap();
    let profiles = tempdir.path().join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::env::set_var("PROTOCOL_PROFILES_DIR", &profiles);
    std::env::set_var("PROTOCOL_LEDGER_DIR", tempdir.path().join("ledgers"));

    // A real, valid profile OUTSIDE the confined tree...
    let outside = tempdir.path().join("outside.yaml");
    std::fs::write(&outside, minimal_profile_yaml()).unwrap();
    // ...and a symlink INSIDE the tree pointing at it.
    let link = profiles.join("link.yaml");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    let server = ProtocolServer::new();
    let rejected = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: link.to_string_lossy().into_owned(),
            session_id: Some("sess-symlink".to_string()),
            initial_context: None,
        }))
        .await;
    assert!(
        rejected.is_err(),
        "a symlink resolving outside PROTOCOL_PROFILES_DIR must be rejected"
    );
}

/// Full artifact-offload journey: start → advance to the checklist → offload an
/// artifact through the enforcer (enforcer-authored sha256) → complete. The
/// completion-time integrity check passes because the stored blob is unchanged.
#[tokio::test]
async fn artifact_offload_then_completion_verifies_ok() {
    let _env = ScratchEnv::enter().await;
    let tempdir = tempfile::TempDir::new().unwrap();
    std::env::set_var("PROTOCOL_LEDGER_DIR", tempdir.path().join("ledgers"));
    std::env::set_var("PROTOCOL_ARTIFACT_DIR", tempdir.path().join("art"));

    let manifest = tempdir.path().join("p.yaml");
    std::fs::write(&manifest, minimal_profile_yaml()).unwrap();
    let server = ProtocolServer::new();
    let sid = "sess-artifact-ok";

    // start → phase1/intro
    server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: manifest.to_string_lossy().into_owned(),
            session_id: Some(sid.to_string()),
            initial_context: None,
        }))
        .await
        .expect("start");
    // advance intro → gate (checklist)
    server
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: sid.to_string(),
            macro_id: "phase1".to_string(),
            sub_state_id: "intro".to_string(),
            evidence: None,
            output: None,
        }))
        .await
        .expect("advance to gate");

    // offload an artifact — sha256 is enforcer-authored
    let stored = server
        .protocol_store_artifact(Parameters(ProtocolStoreArtifactRequest {
            session_id: sid.to_string(),
            step_id: "gate".to_string(),
            content: "the evidence bytes".to_string(),
            mime_type: None,
        }))
        .await
        .expect("store_artifact");
    assert_eq!(stored.0["mime_type"], "text/plain");
    assert!(stored.0["sha256"].as_str().unwrap().len() == 64);

    // complete the final checklist — artifact integrity is re-verified here
    let done = server
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: sid.to_string(),
            macro_id: "phase1".to_string(),
            sub_state_id: "gate".to_string(),
            evidence: Some(std::collections::HashMap::from([(
                "a".to_string(),
                serde_json::json!(true),
            )])),
            output: None,
        }))
        .await
        .expect("final submit must complete");
    assert_eq!(done.0["session_state"], "completed");
}

/// A tampered artifact blocks completion: after offload, overwriting the stored
/// blob makes the enforcer-authored sha256 no longer match, so the
/// completion-time integrity check fails and the session does NOT complete.
#[tokio::test]
async fn tampered_artifact_blocks_completion() {
    let _env = ScratchEnv::enter().await;
    let tempdir = tempfile::TempDir::new().unwrap();
    std::env::set_var("PROTOCOL_LEDGER_DIR", tempdir.path().join("ledgers"));
    let art = tempdir.path().join("art");
    std::env::set_var("PROTOCOL_ARTIFACT_DIR", &art);

    let manifest = tempdir.path().join("p.yaml");
    std::fs::write(&manifest, minimal_profile_yaml()).unwrap();
    let server = ProtocolServer::new();
    let sid = "sess-artifact-tamper";

    server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: manifest.to_string_lossy().into_owned(),
            session_id: Some(sid.to_string()),
            initial_context: None,
        }))
        .await
        .expect("start");
    server
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: sid.to_string(),
            macro_id: "phase1".to_string(),
            sub_state_id: "intro".to_string(),
            evidence: None,
            output: None,
        }))
        .await
        .expect("advance to gate");

    let stored = server
        .protocol_store_artifact(Parameters(ProtocolStoreArtifactRequest {
            session_id: sid.to_string(),
            step_id: "gate".to_string(),
            content: "original".to_string(),
            mime_type: None,
        }))
        .await
        .expect("store_artifact");

    // Tamper: overwrite the stored blob (path is relative to the artifact root).
    let rel = stored.0["path"].as_str().unwrap();
    std::fs::write(art.join(rel), "TAMPERED").unwrap();

    let result = server
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: sid.to_string(),
            macro_id: "phase1".to_string(),
            sub_state_id: "gate".to_string(),
            evidence: Some(std::collections::HashMap::from([(
                "a".to_string(),
                serde_json::json!(true),
            )])),
            output: None,
        }))
        .await;
    assert!(
        result.is_err(),
        "a tampered artifact must block session completion"
    );
}

/// The `ArtifactStored` event survives a restart: a SECOND `ProtocolServer`
/// (fresh in-memory map, same `LEDGER_DIR`) recovers the session from the ledger
/// — rehydrating `session.artifacts` via the recovery fold — and the
/// completion-time integrity check still passes against the recovered set.
#[tokio::test]
async fn artifact_survives_recovery_and_completes() {
    let _env = ScratchEnv::enter().await;
    let tempdir = tempfile::TempDir::new().unwrap();
    std::env::set_var("PROTOCOL_LEDGER_DIR", tempdir.path().join("ledgers"));
    std::env::set_var("PROTOCOL_ARTIFACT_DIR", tempdir.path().join("art"));
    let manifest = tempdir.path().join("p.yaml");
    std::fs::write(&manifest, minimal_profile_yaml()).unwrap();
    let sid = "sess-artifact-recover";

    // Server A: start, advance to the gate, offload an artifact.
    {
        let a = ProtocolServer::new();
        a.protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: manifest.to_string_lossy().into_owned(),
            session_id: Some(sid.to_string()),
            initial_context: None,
        }))
        .await
        .expect("start");
        a.protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: sid.to_string(),
            macro_id: "phase1".to_string(),
            sub_state_id: "intro".to_string(),
            evidence: None,
            output: None,
        }))
        .await
        .expect("advance");
        a.protocol_store_artifact(Parameters(ProtocolStoreArtifactRequest {
            session_id: sid.to_string(),
            step_id: "gate".to_string(),
            content: "survives restart".to_string(),
            mime_type: None,
        }))
        .await
        .expect("store");
    } // drop A — its in-memory session map is gone.

    // Server B: fresh map. Completing forces a cache-miss recovery that must
    // replay ArtifactStored and re-verify the rehydrated artifact.
    let b = ProtocolServer::new();
    let done = b
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: sid.to_string(),
            macro_id: "phase1".to_string(),
            sub_state_id: "gate".to_string(),
            evidence: Some(std::collections::HashMap::from([(
                "a".to_string(),
                serde_json::json!(true),
            )])),
            output: None,
        }))
        .await
        .expect("recovered session must complete");
    assert_eq!(done.0["session_state"], "completed");
}

/// `PROTOCOL_MANIFEST_STRICT=1` rejects a path-mode `manifest_path` (here, an
/// absolute `.yaml` path) even though it would otherwise resolve and load
/// fine; a bare profile NAME still starts OK under the same env. With the
/// var unset, the identical path-mode call still works (non-regression).
#[tokio::test]
async fn strict_mode_rejects_path_but_allows_bare_name() {
    let _env = ScratchEnv::enter().await;
    let tempdir = tempfile::TempDir::new().unwrap();
    let profiles = tempdir.path().join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::env::set_var("PROTOCOL_PROFILES_DIR", &profiles);
    std::env::set_var("PROTOCOL_LEDGER_DIR", tempdir.path().join("ledgers"));
    std::fs::write(profiles.join("named.yaml"), minimal_profile_yaml()).unwrap();

    let outside = tempdir.path().join("outside.yaml");
    std::fs::write(&outside, minimal_profile_yaml()).unwrap();

    std::env::set_var("PROTOCOL_MANIFEST_STRICT", "1");
    let server = ProtocolServer::new();

    let path_rejected = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: outside.to_string_lossy().into_owned(),
            session_id: Some("sess-strict-path".to_string()),
            initial_context: None,
        }))
        .await;
    assert!(
        path_rejected.is_err(),
        "a path-mode manifest_path must be rejected when PROTOCOL_MANIFEST_STRICT is set"
    );

    let name_ok = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: "named".to_string(),
            session_id: Some("sess-strict-name".to_string()),
            initial_context: None,
        }))
        .await;
    assert!(
        name_ok.is_ok(),
        "a bare profile NAME must still start when PROTOCOL_MANIFEST_STRICT is set, got: {:?}",
        name_ok.err()
    );

    // Non-regression: with strict mode unset (and PROTOCOL_PROFILES_DIR
    // confinement also unset, so only the strict-mode behavior is under
    // test), the same path-mode call works.
    std::env::remove_var("PROTOCOL_MANIFEST_STRICT");
    std::env::remove_var("PROTOCOL_PROFILES_DIR");
    let path_ok = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: outside.to_string_lossy().into_owned(),
            session_id: Some("sess-nostrict-path".to_string()),
            initial_context: None,
        }))
        .await;
    assert!(
        path_ok.is_ok(),
        "a path-mode manifest_path must still work when PROTOCOL_MANIFEST_STRICT is unset, got: {:?}",
        path_ok.err()
    );
}

/// Two concurrent `protocol_start` calls for the SAME session_id race to
/// atomically claim the ledger file: exactly one must win (Ok), the other
/// must lose with an "already exists" error, and the ledger must contain
/// exactly ONE `SessionStarted` event -- proving the claim is a real
/// cross-process-safe atomic gate, not just the in-memory map check.
#[tokio::test]
async fn concurrent_start_same_session_id_is_atomic() {
    let _env = ScratchEnv::enter().await;
    let tempdir = tempfile::TempDir::new().unwrap();
    std::env::set_var("PROTOCOL_LEDGER_DIR", tempdir.path().join("ledgers"));

    let manifest = tempdir.path().join("p.yaml");
    std::fs::write(&manifest, minimal_profile_yaml()).unwrap();

    let server_a = ProtocolServer::new();
    let server_b = server_a.clone();
    let manifest_a = manifest.to_string_lossy().into_owned();
    let manifest_b = manifest_a.clone();
    let sid = "sess-race";

    let (r1, r2) = tokio::join!(
        server_a.protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: manifest_a,
            session_id: Some(sid.to_string()),
            initial_context: None,
        })),
        server_b.protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: manifest_b,
            session_id: Some(sid.to_string()),
            initial_context: None,
        })),
    );

    let ok_count = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
    let err_count = [&r1, &r2].iter().filter(|r| r.is_err()).count();
    assert_eq!(ok_count, 1, "exactly one concurrent start must succeed");
    assert_eq!(err_count, 1, "exactly one concurrent start must fail");
    let err_msg = if r1.is_err() {
        r1.err().unwrap().message
    } else {
        r2.err().unwrap().message
    };
    assert!(
        err_msg.contains("already exists"),
        "the losing start must report 'already exists', got: {err_msg}"
    );

    let events = protocol_ledger::Ledger::replay(sid, &tempdir.path().join("ledgers")).unwrap();
    let started_count = events
        .iter()
        .filter(|e| {
            matches!(
                e.event_type,
                protocol_types::FsmEventType::SessionStarted { .. }
            )
        })
        .count();
    assert_eq!(
        started_count, 1,
        "the ledger must contain exactly one SessionStarted event"
    );
}

/// A failed `protocol_start` (bad/nonexistent manifest) must clean up the
/// empty ledger file created by the atomic claim -- otherwise a second,
/// corrected `protocol_start` with the SAME session_id would be permanently
/// blocked by the leftover empty claim file.
#[tokio::test]
async fn failed_start_cleans_up_claim_for_retry() {
    let _env = ScratchEnv::enter().await;
    let tempdir = tempfile::TempDir::new().unwrap();
    std::env::set_var("PROTOCOL_LEDGER_DIR", tempdir.path().join("ledgers"));

    let good_manifest = tempdir.path().join("good.yaml");
    std::fs::write(&good_manifest, minimal_profile_yaml()).unwrap();
    let bad_manifest = tempdir.path().join("does-not-exist.yaml");

    let server = ProtocolServer::new();
    let sid = "sess-retry-after-cleanup";

    let bad = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: bad_manifest.to_string_lossy().into_owned(),
            session_id: Some(sid.to_string()),
            initial_context: None,
        }))
        .await;
    assert!(bad.is_err(), "starting with a bad manifest must fail");

    let retry = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: good_manifest.to_string_lossy().into_owned(),
            session_id: Some(sid.to_string()),
            initial_context: None,
        }))
        .await;
    assert!(
        retry.is_ok(),
        "a retry with the same session_id and a good manifest must succeed \
         once the empty claim file from the failed attempt is cleaned up, got: {:?}",
        retry.err()
    );
}
