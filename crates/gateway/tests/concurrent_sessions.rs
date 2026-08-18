//! Proof: the enforcer governs N concurrent sub-agent sessions independently.
//!
//! The Protocol Enforcer exists to force-order *sub-agents*' work, and the
//! engine drives "any number of concurrent sessions" (see the module doc on
//! `ProfileFsmEngine`). This test makes that concrete: three sub-agents (three
//! distinct `session_id`s) run against ONE `ProtocolServer` at the same time,
//! their steps interleaved. Each is forced through its own state chain, and
//! advancing one never moves another — session isolation is the property a
//! multi-sub-agent orchestrator relies on. Deterministic, in-process, no LLM.

use rmcp::handler::server::wrapper::Parameters;

use protocol_gateway::server::{
    ProtocolGetStateRequest, ProtocolServer, ProtocolStartRequest, ProtocolSubmitMilestoneRequest,
};

/// Two macros: `draft` (inject `s1` -> checklist `chk1`) then `ship` (final
/// checklist `chk2`). Identical for every sub-agent; only the session_id differs.
fn write_profile(dir: &std::path::Path) -> String {
    let profile_path = dir.join("profile.yaml");
    std::fs::write(
        &profile_path,
        r#"
name: "concurrent-sessions-test"
version: "1.0.0"
description: "N concurrent sub-agent sessions"

pipeline:
  - state_id: "draft"
    name: "Draft"
    max_iterations: 5
    sub_states:
      - id: "s1"
        type: inject
        name: "Step 1"
        inject:
          prompt: "do step 1"
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
    )
    .unwrap();
    profile_path.to_string_lossy().into_owned()
}

/// `std::env::set_current_dir` is process-global; serialize cwd-scoped tests.
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

async fn start(server: &ProtocolServer, manifest: &str, sid: &str) {
    let r = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: manifest.to_string(),
            session_id: Some(sid.to_string()),
            initial_context: None,
        }))
        .await
        .expect("start");
    assert_eq!(r.0["position"]["sub_state_id"], "s1", "{sid} starts at s1");
}

/// Submit a plain (non-checklist) advance or a checklist with `criterion`.
async fn submit(
    server: &ProtocolServer,
    sid: &str,
    macro_id: &str,
    sub: &str,
    criterion: Option<&str>,
) -> serde_json::Value {
    let evidence = criterion
        .map(|c| std::collections::HashMap::from([(c.to_string(), serde_json::json!(true))]));
    server
        .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: sid.to_string(),
            macro_id: macro_id.to_string(),
            sub_state_id: sub.to_string(),
            evidence,
            output: None,
        }))
        .await
        .expect("submit")
        .0
}

/// Assert `sid`'s current macro/sub via protocol_get_state (or "completed").
async fn assert_at(server: &ProtocolServer, sid: &str, macro_id: &str, sub: &str) {
    let s = server
        .protocol_get_state(Parameters(ProtocolGetStateRequest {
            session_id: sid.to_string(),
        }))
        .await
        .expect("get_state")
        .0;
    assert_eq!(s["session_state"], "active", "{sid} active");
    assert_eq!(s["position"]["macro_id"], macro_id, "{sid} macro");
    assert_eq!(s["position"]["sub_state_id"], sub, "{sid} sub");
}

async fn assert_completed(server: &ProtocolServer, sid: &str) {
    let s = server
        .protocol_get_state(Parameters(ProtocolGetStateRequest {
            session_id: sid.to_string(),
        }))
        .await
        .expect("get_state")
        .0;
    assert_eq!(s["session_state"], "completed", "{sid} completed");
}

#[tokio::test]
async fn enforcer_governs_n_concurrent_subagent_sessions_independently() {
    let scratch = ScratchCwd::enter().await;
    let manifest = write_profile(scratch.path());
    let server = ProtocolServer::new();

    // Three sub-agents open sessions against the SAME enforcer at once.
    start(&server, &manifest, "agent-a").await;
    start(&server, &manifest, "agent-b").await;
    start(&server, &manifest, "agent-c").await;

    // Interleave: advance a and b past s1; leave c untouched at s1.
    submit(&server, "agent-a", "draft", "s1", None).await;
    submit(&server, "agent-b", "draft", "s1", None).await;
    // Independent positions — b's/a's progress did not drag c along.
    assert_at(&server, "agent-a", "draft", "chk1").await;
    assert_at(&server, "agent-b", "draft", "chk1").await;
    assert_at(&server, "agent-c", "draft", "s1").await;

    // a clears the draft checklist and crosses into the next macro.
    let r = submit(&server, "agent-a", "draft", "chk1", Some("done")).await;
    assert_eq!(r["position"]["macro_id"], "ship");
    assert_eq!(r["position"]["sub_state_id"], "chk2");
    // ISOLATION: a's macro transition left b and c exactly where they were.
    assert_at(&server, "agent-a", "ship", "chk2").await;
    assert_at(&server, "agent-b", "draft", "chk1").await;
    assert_at(&server, "agent-c", "draft", "s1").await;

    // a finishes entirely; b and c must be unaffected by a's completion.
    let done = submit(&server, "agent-a", "ship", "chk2", Some("shipped")).await;
    assert_eq!(done["advanced"], true);
    assert_eq!(done["session_state"], "completed");
    assert_completed(&server, "agent-a").await;
    assert_at(&server, "agent-b", "draft", "chk1").await;
    assert_at(&server, "agent-c", "draft", "s1").await;

    // b and c are still fully live and independently governed to the end.
    submit(&server, "agent-c", "draft", "s1", None).await; // c: s1 -> chk1
    assert_at(&server, "agent-c", "draft", "chk1").await;
    submit(&server, "agent-b", "draft", "chk1", Some("done")).await; // b -> ship/chk2
    assert_at(&server, "agent-b", "ship", "chk2").await;
    let b_done = submit(&server, "agent-b", "ship", "chk2", Some("shipped")).await;
    assert_eq!(b_done["session_state"], "completed");
    // c never moved past chk1 while a and b both completed — full isolation.
    assert_at(&server, "agent-c", "draft", "chk1").await;
}

/// Two concurrent `submit_milestone` calls naming the SAME session at the SAME
/// checklist position must serialize on the per-session lock: exactly one
/// advances, the other sees the moved position and is rejected. Never a
/// double-advance or a stale-copy lost update. Regression guard for the fix
/// that stopped removing the engine from the registry across the blocking call
/// (the old code let a racing call recover a stale engine from the ledger and
/// advance a second time).
#[tokio::test]
async fn concurrent_same_session_submits_serialize_no_double_advance() {
    let scratch = ScratchCwd::enter().await;
    let manifest = write_profile(scratch.path());
    let server = ProtocolServer::new();

    start(&server, &manifest, "racer").await;
    submit(&server, "racer", "draft", "s1", None).await; // s1 -> draft/chk1

    // Fire two identical checklist submissions for (draft, chk1) at once. Each
    // awaits its own blocking thread, so the two run genuinely in parallel.
    let mk = || {
        let evidence =
            std::collections::HashMap::from([("done".to_string(), serde_json::json!(true))]);
        server.protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
            session_id: "racer".to_string(),
            macro_id: "draft".to_string(),
            sub_state_id: "chk1".to_string(),
            evidence: Some(evidence),
            output: None,
        }))
    };
    let (r1, r2) = tokio::join!(mk(), mk());

    // Exactly one may advance; the other is rejected (position already moved).
    let advanced: Vec<_> = [r1, r2]
        .into_iter()
        .filter_map(|r| r.ok())
        .filter(|j| j.0["advanced"] == serde_json::json!(true))
        .collect();
    assert_eq!(
        advanced.len(),
        1,
        "exactly one concurrent submit may advance the checklist (no double-advance)"
    );
    assert_eq!(advanced[0].0["position"]["macro_id"], "ship");

    // The session advanced EXACTLY ONCE and is internally consistent afterward.
    assert_at(&server, "racer", "ship", "chk2").await;
}
