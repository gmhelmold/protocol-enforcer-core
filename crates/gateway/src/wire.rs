// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! MCP wire-shape serializers: turn engine `StepView` values into the JSON
//! response bodies the MCP tools return. Split out of `server.rs` to keep it
//! under the size cap. Mostly pure formatting; the two notary helpers at the
//! bottom (`profile_sha256_of`, `transcript_root_best_effort`) do a single
//! best-effort read each and live here to keep `server.rs` under the cap.

use std::path::Path;

use rmcp::ErrorData;
use sha2::{Digest, Sha256};

use protocol_fsm::{RenderedInjection, StepView};
use protocol_ledger::{merkle, Ledger};
use protocol_types::{FsmError, Position, SessionStatus};

use crate::error_map::fsm_error_to_mcp;
use crate::paths::ledger_dir;

/// The `outputSchema` advertised for every tool: a permissive `{"type":"object"}`.
///
/// rmcp otherwise DERIVES each tool's output schema from its `Json<T>` return
/// type; for `T = serde_json::Value` that schema is the empty schema
/// (`{"$schema": ...}` with no `"type"`), which Claude Code's MCP client rejects
/// ("expected object at tools.N.outputSchema.type"), making every tool unusable
/// from a Claude session. Passing this explicitly via each
/// `#[tool(output_schema = ...)]` overrides the return-type derivation, so the
/// tools keep returning `Json<serde_json::Value>` (and their in-process test
/// ergonomics) unchanged. The bodies really are dynamic JSON objects, so a bare
/// object schema is the honest, minimal shape.
pub(crate) fn object_output_schema() -> std::sync::Arc<rmcp::model::JsonObject> {
    let mut schema = serde_json::Map::new();
    schema.insert(
        "type".to_string(),
        serde_json::Value::String("object".to_string()),
    );
    std::sync::Arc::new(schema)
}

pub(crate) fn session_status_str(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
        SessionStatus::Completed => "completed",
        SessionStatus::Failed => "failed",
    }
}

/// The step payload. `approval_challenge`/`approval_prompt` are OMITTED (not
/// null) on every sub-state that is not a `human_approval` gate, so a client can
/// treat "the key is there" as "a human must sign this step" without a magic
/// sentinel — and every pre-feature response stays byte-identical on the wire.
pub(crate) fn injected_json(injected: &RenderedInjection) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "prompt": injected.prompt,
        "skill_ref": injected.skill_ref,
        "protocol_ref": injected.protocol_ref,
        "context": injected.context,
        "rendered": injected.rendered,
    });
    if let Some(challenge) = &injected.approval_challenge {
        obj["approval_challenge"] = serde_json::Value::String(challenge.clone());
    }
    if let Some(prompt) = &injected.approval_prompt {
        obj["approval_prompt"] = serde_json::Value::String(prompt.clone());
    }
    obj
}

pub(crate) fn macro_json(position: &Position, macro_name: &str) -> serde_json::Value {
    serde_json::json!({
        "state_id": position.macro_id,
        "name": macro_name,
    })
}

/// Wraps a `StepView` into the `protocol_start` response shape
/// (adds `session_id`, expands `macro{state_id,name}`, lowercases
/// `session_state`).
pub(crate) fn start_response_json(session_id: &str, view: &StepView) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "position": view.position,
        "injected": injected_json(&view.injected),
        "macro": macro_json(&view.position, &view.macro_name),
        "session_state": session_status_str(&view.session_state),
    })
}

/// Wraps a `StepView` into the `protocol_submit_milestone` advance
/// response shape.
pub(crate) fn advance_response_json(view: &StepView) -> serde_json::Value {
    serde_json::json!({
        "advanced": true,
        "position": view.position,
        "injected": injected_json(&view.injected),
        "macro": macro_json(&view.position, &view.macro_name),
        "session_state": session_status_str(&view.session_state),
    })
}

/// Wraps a `StepView` into the macro loop-back `looped_back` response
/// shape. Deliberately NOT built on `advance_response_json` (which
/// hardcodes `"advanced": true` -- a loop-back IS a rejection, criteria not
/// met). `protocol_submit_milestone`'s docstring contract is
/// `advanced:true`=success, `advanced:false`=rejection; a consumer doing
/// `if response.advanced` must see `false` here, with `looped_back: true`
/// as the explicit signal that the position moved anyway.
pub(crate) fn looped_back_response_json(
    view: &StepView,
    rejected_items: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "advanced": false,
        "looped_back": true,
        "rejected_items": rejected_items,
        "position": view.position,
        "injected": injected_json(&view.injected),
        "macro": macro_json(&view.position, &view.macro_name),
        "session_state": session_status_str(&view.session_state),
    })
}

pub(crate) fn session_not_found_error(session_id: &str) -> ErrorData {
    fsm_error_to_mcp(FsmError::SessionNotFound(session_id.to_string()))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// SHA-256 (lowercase hex) of the profile file bytes at `path` — the bytes that
/// were actually loaded. Best-effort: a read
/// failure logs and yields `None` rather than failing the start/recovery, which
/// would turn an audit feature into an availability regression. In practice the
/// profile has just loaded, so the read succeeds.
pub(crate) fn profile_sha256_of(path: &Path) -> Option<String> {
    match std::fs::read(path) {
        Ok(bytes) => Some(format!("{:x}", Sha256::digest(&bytes))),
        Err(e) => {
            tracing::warn!("profile_sha256: cannot read profile at {:?}: {}", path, e);
            None
        }
    }
}

/// RFC 6962 transcript root over the session's now-final ledger. Called
/// from the completion arm; best-effort (NOT-9/NOT-10) — if the ledger cannot be
/// read the root is omitted and a warning is logged, never failing a session
/// that already completed.
pub(crate) fn transcript_root_best_effort(session_id: &str) -> Option<String> {
    transcript_root_in(session_id, &ledger_dir())
}

/// Inner form taking an explicit ledger dir, so it is testable without touching
/// the `PROTOCOL_LEDGER_DIR` env (which parallel tests would race on).
fn transcript_root_in(session_id: &str, dir: &Path) -> Option<String> {
    match Ledger::lines(session_id, dir) {
        Ok(leaves) => Some(to_hex(&merkle::root(&leaves))),
        Err(e) => {
            tracing::warn!(
                "transcript_root: cannot read ledger for '{}': {}",
                session_id,
                e
            );
            None
        }
    }
}

/// The `protocol_submit_milestone` completion response (§6). The
/// `transcript_root` key is OMITTED (not null) when it could not be computed —
/// best-effort, NOT-10 — so a consumer can tell "no root issued" from a present
/// value without a magic sentinel.
pub(crate) fn completed_response_json(
    contract_path: Option<String>,
    transcript_root: Option<String>,
) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "advanced": true,
        "position": serde_json::Value::Null,
        "injected": serde_json::Value::Null,
        "session_state": "completed",
        "contract_written": contract_path.is_some(),
        "contract_path": contract_path,
    });
    if let Some(root) = transcript_root {
        obj["transcript_root"] = serde_json::Value::String(root);
    }
    obj
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn profile_sha256_of_matches_independent_digest() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("profile.yaml");
        std::fs::write(&p, b"name: demo\nversion: 1\n").unwrap();
        let expected = format!("{:x}", Sha256::digest(b"name: demo\nversion: 1\n"));
        assert_eq!(profile_sha256_of(&p), Some(expected));
    }

    #[test]
    fn profile_sha256_of_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(profile_sha256_of(&dir.path().join("nope.yaml")), None);
    }

    #[test]
    fn transcript_root_matches_merkle_root_over_the_same_ledger() {
        // The root the completion arm issues equals
        // `merkle::root(Ledger::lines(..))` computed directly over the ledger.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("s.jsonl"), b"line-a\nline-b\nline-c\n").unwrap();
        let leaves = Ledger::lines("s", dir.path()).unwrap();
        let expected = to_hex(&merkle::root(&leaves));
        assert_eq!(transcript_root_in("s", dir.path()), Some(expected));
    }

    #[test]
    fn transcript_root_missing_ledger_is_none() {
        // NOT-10: a failed read yields None (the field is then omitted upstream),
        // never a crash and never a well-formed root over nothing.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(transcript_root_in("absent", dir.path()), None);
    }

    #[test]
    fn completed_response_includes_root_when_present() {
        let r = completed_response_json(Some("out.json".into()), Some("deadbeef".into()));
        assert_eq!(r["transcript_root"], serde_json::json!("deadbeef"));
        assert_eq!(r["contract_written"], serde_json::json!(true));
    }

    #[test]
    fn completed_response_omits_root_when_absent() {
        let r = completed_response_json(None, None);
        // Key absent (not null) — NOT-10.
        assert!(r.get("transcript_root").is_none());
        assert_eq!(r["contract_written"], serde_json::json!(false));
    }
}
