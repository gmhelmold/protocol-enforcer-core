// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter

//! #9 wire_loopback (macro loop-back acceptance test): the gateway's
//! `protocol_submit_milestone` response for a macro loop-back has
//! `advanced:false`, `looped_back:true`, `rejected_items`, PLUS the new
//! position/injected/macro fields -- built by `looped_back_response_json`,
//! deliberately NOT `advance_response_json` (which hardcodes
//! `advanced:true`).

use rmcp::handler::server::wrapper::Parameters;
use std::collections::HashMap;

use protocol_gateway::server::{
    ProtocolServer, ProtocolStartRequest, ProtocolSubmitMilestoneRequest,
};

/// A single loop-enabled macro (`draft`, loop: true, execute sub `work`)
/// followed by a final, non-loop macro (`ship`).
fn write_loop_profile(dir: &std::path::Path) -> String {
    let profile_path = dir.join("profile.yaml");
    let profile_yaml = r#"
name: "wire-loopback-test"
version: "1.0.0"
description: "gateway wire_loopback test profile"

settings:
  auto_loop_on_checklist_failure: true

pipeline:
  - state_id: "draft"
    name: "Draft"
    loop: true
    sub_states:
      - id: "intro"
        type: inject
        name: "Intro"
      - id: "work"
        type: execute
        name: "Work"
      - id: "gate"
        type: checklist
        name: "Gate"
        criteria:
          - "a"
          - "b"

  - state_id: "ship"
    name: "Ship"
    sub_states:
      - id: "final_gate"
        type: checklist
        name: "Final Gate"
        criteria:
          - "c"
"#;
    std::fs::write(&profile_path, profile_yaml).unwrap();
    profile_path.to_string_lossy().into_owned()
}

/// cwd-scopes the process so `./ledger` lands inside a throwaway tempdir
/// (mirrors `session_id_validation.rs`'s `ScratchCwd`).
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

#[tokio::test]
async fn wire_loopback() {
    let scratch = ScratchCwd::enter().await;
    let manifest_path = write_loop_profile(scratch.path());
    let server = ProtocolServer::new();

    let start = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path,
            session_id: Some("wire-loopback-sess".to_string()),
            initial_context: None,
        }))
        .await
        .expect("protocol_start");
    let session_id = start.0["session_id"].as_str().unwrap().to_string();
    assert_eq!(start.0["position"]["sub_state_id"], "intro");

    // Plain ack through "intro" -> "work".
    let after_intro = server
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: session_id.clone(),
            macro_id: "draft".to_string(),
            sub_state_id: "intro".to_string(),
            evidence: None,
            output: None,
        }))
        .await
        .expect("plain ack");
    assert_eq!(after_intro.0["position"]["sub_state_id"], "work");

    // Plain ack through "work" -> "gate".
    let after_work = server
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: session_id.clone(),
            macro_id: "draft".to_string(),
            sub_state_id: "work".to_string(),
            evidence: None,
            output: None,
        }))
        .await
        .expect("plain ack");
    assert_eq!(after_work.0["position"]["sub_state_id"], "gate");

    // Reject the checklist (missing "b") -> must loop back.
    let mut evidence = HashMap::new();
    evidence.insert("a".to_string(), serde_json::json!(true));
    let response = server
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: session_id.clone(),
            macro_id: "draft".to_string(),
            sub_state_id: "gate".to_string(),
            evidence: Some(evidence),
            output: None,
        }))
        .await
        .expect("rejection must not error")
        .0;

    assert_eq!(
        response["advanced"],
        serde_json::Value::Bool(false),
        "a loop-back IS a rejection: advanced must be false"
    );
    assert_eq!(response["looped_back"], serde_json::Value::Bool(true));
    assert_eq!(
        response["rejected_items"],
        serde_json::json!(["b"]),
        "must carry WHY it rejected"
    );
    assert_eq!(
        response["position"]["macro_id"], "draft",
        "must carry the NEW position"
    );
    assert_eq!(response["position"]["sub_state_id"], "work");
    assert!(
        !response["injected"].is_null(),
        "must carry the new position's injection"
    );
    assert_eq!(response["macro"]["state_id"], "draft");
}
