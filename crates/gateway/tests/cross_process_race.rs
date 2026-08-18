// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Real cross-PROCESS validation of the two `protocol_start` hardening changes
//! (PR #44), driving actual spawned `protocol-gateway` binaries over genuine
//! MCP stdio — NOT the in-process `ProtocolServer` the unit tests use. The
//! unit test `concurrent_start_same_session_id_is_atomic` shares ONE
//! `ProtocolServer` (one in-memory session map), so it can only prove the
//! in-process layer; the headline of ADR-33 is that the atomic `create_new`
//! ledger claim arbitrates across SEPARATE PROCESSES sharing one
//! `PROTOCOL_LEDGER_DIR`. That is what this test exercises for real.

use rmcp::model::CallToolRequestParams;
use rmcp::ServiceExt;

fn minimal_profile() -> &'static str {
    r#"
name: "cross-process-race"
version: "1.0.0"
description: "cross-process start-race + strict-mode wire test"

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

/// Spawn a real `protocol-gateway` child bound to the given ledger/artifact
/// dirs (and optional extra env), returning a connected raw MCP client.
async fn spawn_gateway(
    ledger_dir: &std::path::Path,
    artifact_dir: &std::path::Path,
    extra_env: &[(&str, String)],
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_protocol-gateway"));
    cmd.env("PROTOCOL_LEDGER_DIR", ledger_dir);
    cmd.env("PROTOCOL_ARTIFACT_DIR", artifact_dir);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let transport = rmcp::transport::TokioChildProcess::new(cmd).expect("spawn protocol-gateway");
    ().serve(transport)
        .await
        .expect("connect to the real gateway over stdio")
}

/// Fire `protocol_start` for a fixed `manifest`/`session_id` over one wire.
async fn start_same(
    svc: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    manifest: &str,
    sid: &str,
) -> Result<rmcp::model::CallToolResult, rmcp::ServiceError> {
    let params = CallToolRequestParams::new("protocol_start").with_arguments(
        serde_json::json!({ "manifest_path": manifest, "session_id": sid })
            .as_object()
            .cloned()
            .unwrap(),
    );
    svc.call_tool(params).await
}

/// Was a `protocol_start` over the wire a real success? True only when the
/// call returned a non-error tool result with structured content.
fn is_success(result: &Result<rmcp::model::CallToolResult, rmcp::ServiceError>) -> bool {
    match result {
        Ok(r) => r.is_error != Some(true) && r.structured_content.is_some(),
        Err(_) => false,
    }
}

/// Two SEPARATE gateway processes share one `PROTOCOL_LEDGER_DIR` and fire
/// `protocol_start` for the SAME `session_id` concurrently. The atomic
/// `create_new` ledger claim (ADR-33) must let exactly one win; the loser must
/// fail; and the shared on-disk ledger must contain exactly ONE
/// `SessionStarted` — no cross-process double-start.
#[tokio::test]
async fn two_processes_racing_same_session_id_yield_one_winner() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let ledger_dir = tempdir.path().join("ledgers");
    let artifact_dir = tempdir.path().join("art");
    let manifest = tempdir.path().join("p.yaml");
    std::fs::write(&manifest, minimal_profile()).unwrap();
    let manifest_arg = manifest.to_string_lossy().into_owned();
    let sid = "cross-proc-race";

    // Two independent child processes, each its own OS process + open fds.
    let svc_a = spawn_gateway(&ledger_dir, &artifact_dir, &[]).await;
    let svc_b = spawn_gateway(&ledger_dir, &artifact_dir, &[]).await;

    let (r_a, r_b) = tokio::join!(
        start_same(&svc_a, &manifest_arg, sid),
        start_same(&svc_b, &manifest_arg, sid),
    );

    let wins = [is_success(&r_a), is_success(&r_b)]
        .iter()
        .filter(|w| **w)
        .count();
    assert_eq!(
        wins, 1,
        "exactly one of two racing PROCESSES must win protocol_start (a={:?}, b={:?})",
        r_a, r_b
    );

    // The shared on-disk ledger must record exactly one SessionStarted.
    let ledger_file = ledger_dir.join(format!("{sid}.jsonl"));
    let contents = std::fs::read_to_string(&ledger_file).expect("shared ledger file must exist");
    let started = contents
        .lines()
        .filter(|l| l.contains("\"SessionStarted\""))
        .count();
    assert_eq!(
        started, 1,
        "the shared ledger must contain exactly one SessionStarted, got {started}:\n{contents}"
    );

    svc_a.cancel().await.ok();
    svc_b.cancel().await.ok();
}

/// A real gateway process started with `PROTOCOL_MANIFEST_STRICT=1` (ADR-32)
/// must reject a path-form `manifest_path` over the wire, yet still accept a
/// bare profile NAME resolved under `PROTOCOL_PROFILES_DIR`.
#[tokio::test]
async fn strict_mode_over_the_wire_rejects_path_allows_name() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let ledger_dir = tempdir.path().join("ledgers");
    let artifact_dir = tempdir.path().join("art");
    let profiles = tempdir.path().join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(profiles.join("named.yaml"), minimal_profile()).unwrap();

    // A path-form manifest living OUTSIDE the profiles dir.
    let outside = tempdir.path().join("outside.yaml");
    std::fs::write(&outside, minimal_profile()).unwrap();

    let svc = spawn_gateway(
        &ledger_dir,
        &artifact_dir,
        &[
            ("PROTOCOL_MANIFEST_STRICT", "1".to_string()),
            (
                "PROTOCOL_PROFILES_DIR",
                profiles.to_string_lossy().into_owned(),
            ),
        ],
    )
    .await;

    // Path form → rejected.
    let path_res = svc
        .call_tool(
            CallToolRequestParams::new("protocol_start").with_arguments(
                serde_json::json!({
                    "manifest_path": outside.to_string_lossy(),
                    "session_id": "strict-path",
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
        )
        .await;
    assert!(
        !is_success(&path_res),
        "strict mode must reject a path-form manifest_path over the wire, got {path_res:?}"
    );

    // Bare name → accepted.
    let name_res = svc
        .call_tool(
            CallToolRequestParams::new("protocol_start").with_arguments(
                serde_json::json!({ "manifest_path": "named", "session_id": "strict-name" })
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await;
    assert!(
        is_success(&name_res),
        "strict mode must still accept a bare profile NAME over the wire, got {name_res:?}"
    );

    svc.cancel().await.ok();
}
