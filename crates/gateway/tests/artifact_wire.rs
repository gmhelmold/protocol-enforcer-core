// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! Real-wire user journey for the artifact-offload feature: spawn the actual
//! `protocol-gateway` binary and drive a full session over the genuine MCP stdio
//! transport (raw `rmcp` client, no in-process shortcut), calling every tool by
//! name exactly as an external MCP client would — including the new
//! `protocol_store_artifact`. Proves the tool serializes/deserializes over the
//! wire and integrates in a start → advance → store → complete story.

use rmcp::model::CallToolRequestParams;
use rmcp::ServiceExt;

fn minimal_profile() -> &'static str {
    r#"
name: "artifact-wire-test"
version: "1.0.0"
description: "real-wire artifact journey"

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

/// Call a tool by name over the real wire; returns its `structured_content`.
async fn call(
    service: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &'static str,
    args: serde_json::Value,
) -> serde_json::Value {
    let obj = args.as_object().cloned().unwrap_or_default();
    let params = CallToolRequestParams::new(name).with_arguments(obj);
    let result = service
        .call_tool(params)
        .await
        .unwrap_or_else(|e| panic!("{name} over the wire failed: {e}"));
    assert_ne!(result.is_error, Some(true), "{name} returned is_error");
    result
        .structured_content
        .unwrap_or_else(|| panic!("{name} had no structured_content"))
}

#[tokio::test]
async fn artifact_offload_full_journey_over_the_real_wire() {
    let tempdir = tempfile::TempDir::new().unwrap();
    let manifest = tempdir.path().join("p.yaml");
    std::fs::write(&manifest, minimal_profile()).unwrap();

    // Spawn the REAL gateway binary, isolating its on-disk state to the tempdir
    // via the child's own env (no process-global env mutation needed).
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_protocol-gateway"));
    cmd.env("PROTOCOL_LEDGER_DIR", tempdir.path().join("ledgers"));
    cmd.env("PROTOCOL_ARTIFACT_DIR", tempdir.path().join("art"));
    let transport = rmcp::transport::TokioChildProcess::new(cmd).expect("spawn protocol-gateway");
    let service = ().serve(transport).await.expect("connect to the real gateway over stdio");

    let sid = "wire-artifact";

    // 1. start → positioned at phase1/intro
    let started = call(
        &service,
        "protocol_start",
        serde_json::json!({
            "manifest_path": manifest.to_string_lossy(),
            "session_id": sid,
        }),
    )
    .await;
    assert_eq!(started["position"]["sub_state_id"], "intro");

    // 2. advance intro → gate
    let advanced = call(
        &service,
        "protocol_submit_milestone",
        serde_json::json!({ "session_id": sid, "macro_id": "phase1", "sub_state_id": "intro" }),
    )
    .await;
    assert_eq!(advanced["position"]["sub_state_id"], "gate");

    // 3. offload an artifact over the wire — enforcer-authored sha256 comes back
    let stored = call(
        &service,
        "protocol_store_artifact",
        serde_json::json!({ "session_id": sid, "step_id": "gate", "content": "wire evidence" }),
    )
    .await;
    assert_eq!(stored["mime_type"], "text/plain");
    assert_eq!(stored["sha256"].as_str().unwrap().len(), 64);
    assert!(stored["size_bytes"].as_u64().unwrap() > 0);

    // 4. complete the final checklist — the artifact integrity is re-verified here
    let done = call(
        &service,
        "protocol_submit_milestone",
        serde_json::json!({
            "session_id": sid,
            "macro_id": "phase1",
            "sub_state_id": "gate",
            "evidence": { "a": true }
        }),
    )
    .await;
    assert_eq!(done["session_state"], "completed");

    // 5. read it back over the wire
    let state = call(
        &service,
        "protocol_get_state",
        serde_json::json!({ "session_id": sid }),
    )
    .await;
    assert_eq!(state["session_state"], "completed");

    service.cancel().await.ok();
}
