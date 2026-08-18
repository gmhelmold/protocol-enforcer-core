//! Gateway memory harness — measures the resident footprint of the real
//! `ProtocolServer` (the served MCP path) at idle vs. after driving M complete
//! sessions through its actual async tool methods (`protocol_start`,
//! `protocol_submit_milestone`).
//!
//! This is the honest gateway-side counterpart to the in-process `throughput`
//! example: unlike that one (single engine, in-memory ledger), the gateway
//! keeps ONE `ProfileFsmEngine` per session in its registry
//! (`HashMap<String, Arc<Mutex<..>>>`) and every step appends to a real on-disk
//! ledger (`Ledger::new` + fsync). So the RSS growth here is the true
//! per-session cost the gateway carries for sessions it has not evicted.
//!
//! CAVEAT: this drives the `ProtocolServer` object IN-PROCESS via its async
//! methods — it does NOT go over the stdio MCP transport, and there is no
//! separate client process. It therefore measures the server's own data
//! structures + per-session engines + ledger I/O, which is the dominant term,
//! but not the rmcp transport/framing overhead of the real `protocol-gateway`
//! binary. Idle RSS of that actual binary is captured separately (see the
//! evidence .md).
//!
//! Run (peak RSS captured by the outer /usr/bin/time -l):
//!   /usr/bin/time -l cargo run --release -p protocol-gateway --example gateway_mem -- [M]
//! Default M=500. Ledgers/artifacts go to a throwaway temp dir that is removed
//! at exit, so this leaves no state behind.

use std::path::{Path, PathBuf};
use std::process::Command;

use protocol_gateway::server::{
    ProtocolServer, ProtocolStartRequest, ProtocolSubmitMilestoneRequest,
};
use rmcp::handler::server::wrapper::{Json, Parameters};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repo root")
}

/// Own resident set size in bytes, via `ps` (macOS reports RSS in KiB).
fn rss_bytes() -> u64 {
    let pid = std::process::id();
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .expect("run ps");
    let kb: u64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("parse ps rss");
    kb * 1024
}

/// default.yaml has one `checklist` sub per macro, each with distinct criteria.
/// Driving the gateway means handing each checklist the evidence it demands;
/// every other sub-state type ignores evidence.
fn criteria_for_macro(macro_id: &str) -> &'static [&'static str] {
    match macro_id {
        "understand" => &[
            "context_gathered",
            "requirements_clear",
            "constraints_identified",
            "dependencies_mapped",
            "risks_documented",
        ],
        "plan" => &[
            "task_decomposed",
            "interfaces_defined",
            "strategy_selected",
            "execution_plan_created",
        ],
        "execute" => &[
            "tests_written",
            "code_implemented",
            "tests_passing",
            "edge_cases_covered",
        ],
        "review" => &[
            "self_review_done",
            "static_analysis_clean",
            "security_checked",
            "performance_verified",
        ],
        "deliver" => &[
            "all_tests_green",
            "code_committed",
            "pr_created",
            "docs_updated",
        ],
        _ => &[],
    }
}

fn evidence_map(
    macro_id: &str,
    sub_state_id: &str,
) -> std::collections::HashMap<String, serde_json::Value> {
    let mut m = std::collections::HashMap::new();
    if sub_state_id == "checklist" {
        for c in criteria_for_macro(macro_id) {
            m.insert((*c).to_string(), serde_json::json!("done"));
        }
    }
    m
}

async fn drive_one(server: &ProtocolServer, profile_path: &str, session_id: &str) -> usize {
    let Json(started) = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: profile_path.to_string(),
            session_id: Some(session_id.to_string()),
            initial_context: None,
        }))
        .await
        .expect("protocol_start");

    let mut macro_id = started["position"]["macro_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut sub_id = started["position"]["sub_state_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut steps = 0usize;

    loop {
        let evidence = evidence_map(&macro_id, &sub_id);
        let Json(resp) = server
            .protocol_submit_milestone(Parameters(ProtocolSubmitMilestoneRequest {
                session_id: session_id.to_string(),
                macro_id: macro_id.clone(),
                sub_state_id: sub_id.clone(),
                evidence: Some(evidence),
                output: None,
            }))
            .await
            .expect("protocol_submit_milestone");
        steps += 1;

        if resp["session_state"] == "completed" {
            break;
        }
        macro_id = resp["position"]["macro_id"].as_str().unwrap().to_string();
        sub_id = resp["position"]["sub_state_id"]
            .as_str()
            .unwrap()
            .to_string();
    }
    steps
}

#[tokio::main]
async fn main() {
    let m: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);

    // Point the gateway's ledger + artifact dirs at a throwaway temp tree.
    let tmp = std::env::temp_dir().join(format!("gw_mem_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("mk temp");
    std::env::set_var("PROTOCOL_LEDGER_DIR", tmp.join("ledger"));
    std::env::set_var("PROTOCOL_ARTIFACT_DIR", tmp.join("artifacts"));

    let profile_path = repo_root()
        .join("profiles/default.yaml")
        .to_string_lossy()
        .into_owned();

    let server = ProtocolServer::new();

    // Idle: a freshly constructed server, empty session registry.
    let idle = rss_bytes();

    // One warm session so any lazy alloc / profile parse path is hot before we
    // read the "after" number's slope.
    let steps_per_session = drive_one(&server, &profile_path, "warm").await;

    let mut total_steps = 0usize;
    for i in 0..m {
        total_steps += drive_one(&server, &profile_path, &format!("m{i}")).await;
    }

    let after = rss_bytes();

    println!("=== gateway memory harness ===");
    println!("profile:            {profile_path}");
    println!("sessions (M):       {m}");
    println!("steps/session:      {steps_per_session}");
    println!("total steps:        {total_steps}");
    println!("--");
    println!(
        "idle RSS:           {idle} bytes ({:.2} MiB)",
        idle as f64 / 1048576.0
    );
    println!(
        "after-M RSS:        {after} bytes ({:.2} MiB)",
        after as f64 / 1048576.0
    );
    println!(
        "delta:              {} bytes ({:.2} MiB)  = {:.1} KiB/session",
        after.saturating_sub(idle),
        after.saturating_sub(idle) as f64 / 1048576.0,
        after.saturating_sub(idle) as f64 / 1024.0 / m as f64
    );

    // Clean up the throwaway ledger/artifact tree.
    let _ = std::fs::remove_dir_all(&tmp);
}
