//! Gateway-owned regression tests for `protocol_start`'s client-supplied
//! `session_id` sanitization (path-traversal -> arbitrary file write fix).
//!
//! `ProtocolServer` writes the ledger and (on completion) output-contract
//! artifacts under fixed relative base dirs (`./ledger`, `./.artifacts`,
//! `./out/...`) keyed by `session_id`. A malicious `session_id` like
//! `"../../evil"` would otherwise let a client write files outside those
//! dirs. These tests assert the strict allow-list validation added to
//! `protocol_start` rejects such ids before any file is touched, while a
//! normal id still works end-to-end.

use rmcp::handler::server::wrapper::Parameters;

use protocol_gateway::server::{ProtocolServer, ProtocolStartRequest};

/// Writes a minimal single-macro profile (an `inject` sub followed by the
/// required terminal `checklist` sub) into `dir` and returns its path.
/// These tests only care about `protocol_start`'s session_id validation.
fn write_minimal_profile(dir: &std::path::Path) -> String {
    let profile_path = dir.join("profile.yaml");
    let profile_yaml = r#"
name: "session-id-validation-test"
version: "1.0.0"
description: "gateway session_id validation test profile"

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
"#;
    std::fs::write(&profile_path, profile_yaml).unwrap();
    profile_path.to_string_lossy().into_owned()
}

/// cwd-scopes the process for the duration of a test so `./ledger` (the
/// fixed base dir `ProtocolServer` writes under) lands inside a throwaway
/// tempdir instead of the repo root. Serialized via a mutex (held across
/// awaits) because `std::env::set_current_dir` is process-global and tests
/// run concurrently.
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

/// Recursively collects every file path under `root` that existed before
/// the test started, to be diffed against what exists after a rejected
/// `protocol_start` call. Used to assert a rejected traversal session_id
/// creates no new file anywhere under the scratch dir (not just under
/// `./ledger`).
fn all_files(root: &std::path::Path) -> std::collections::BTreeSet<std::path::PathBuf> {
    let mut out = std::collections::BTreeSet::new();
    fn walk(dir: &std::path::Path, out: &mut std::collections::BTreeSet<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path);
            }
        }
    }
    walk(root, &mut out);
    out
}

async fn assert_session_id_rejected_and_no_file_written(session_id: &str) {
    let scratch = ScratchCwd::enter().await;
    let manifest_path = write_minimal_profile(scratch.path());
    let before = all_files(scratch.path());

    let server = ProtocolServer::new();
    let result = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path,
            session_id: Some(session_id.to_string()),
            initial_context: None,
        }))
        .await;

    let err = match result {
        Err(e) => e,
        Ok(_) => panic!(
            "session_id {:?} must be rejected by validation, but protocol_start succeeded",
            session_id
        ),
    };
    assert!(
        err.message.contains("session_id"),
        "error message should name the session_id constraint, got: {}",
        err.message
    );

    let after = all_files(scratch.path());
    assert_eq!(
        before, after,
        "a rejected session_id must not create any file (traversal check for {:?})",
        session_id
    );
}

#[tokio::test]
async fn traversal_session_id_is_rejected_with_no_file_written() {
    assert_session_id_rejected_and_no_file_written("../../evil").await;
}

#[tokio::test]
async fn embedded_slash_session_id_is_rejected_with_no_file_written() {
    assert_session_id_rejected_and_no_file_written("a/b").await;
}

#[tokio::test]
async fn whitespace_session_id_is_rejected_with_no_file_written() {
    assert_session_id_rejected_and_no_file_written("x ").await;
}

#[tokio::test]
async fn empty_session_id_is_rejected_with_no_file_written() {
    assert_session_id_rejected_and_no_file_written("").await;
}

#[tokio::test]
async fn overlong_session_id_is_rejected_with_no_file_written() {
    let overlong = "a".repeat(129);
    assert_session_id_rejected_and_no_file_written(&overlong).await;
}

#[tokio::test]
async fn normal_session_id_still_works() {
    let scratch = ScratchCwd::enter().await;
    let manifest_path = write_minimal_profile(scratch.path());

    let server = ProtocolServer::new();
    let result = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path,
            session_id: Some("normal-session_123".to_string()),
            initial_context: None,
        }))
        .await
        .expect("a normal, allow-listed session_id must still be accepted");

    assert_eq!(result.0["session_id"], "normal-session_123");
    assert_eq!(result.0["session_state"], "active");

    // The ledger file for this session was created under the scratch cwd's
    // ./ledger dir, proving normal ids still flow through correctly.
    let ledger_file = scratch
        .path()
        .join("ledger")
        .join("normal-session_123.jsonl");
    assert!(
        ledger_file.exists(),
        "expected ledger file at {:?}",
        ledger_file
    );
}

/// A `manifest_path` with a `..` component is rejected fail-closed before any
/// file is read — `protocol_start` takes a filesystem path by design, but must
/// not let a caller climb out of the intended tree via relative traversal.
#[tokio::test]
async fn manifest_path_with_parent_dir_component_is_rejected() {
    let scratch = ScratchCwd::enter().await;
    let server = ProtocolServer::new();
    let result = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: "../../../../etc/passwd".to_string(),
            session_id: Some("trav".to_string()),
            initial_context: None,
        }))
        .await;

    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("a manifest_path with '..' must be rejected, but protocol_start succeeded"),
    };
    assert!(
        err.message.contains("manifest_path") && err.message.contains(".."),
        "error should name the manifest_path '..' constraint, got: {}",
        err.message
    );
    // The scratch cwd is untouched (no ledger/sidecar written on rejection).
    let _ = scratch;
}

/// A `session_id` whose ledger file already exists on disk (e.g. left over
/// from a prior or completed session, or from another gateway process
/// entirely) must be rejected up front -- otherwise `protocol_start` would
/// append a second `SessionStarted` onto that prior session's ledger. This
/// is distinct from the in-memory `contains_key` check: no `ProtocolServer`
/// instance has this session registered, only the ledger file exists.
#[tokio::test]
async fn session_id_with_existing_ledger_file_is_rejected() {
    let scratch = ScratchCwd::enter().await;
    let manifest_path = write_minimal_profile(scratch.path());

    // Pre-create a ledger file for this session_id, as if a prior process
    // had already started (and possibly completed) a session under it.
    std::fs::create_dir_all(scratch.path().join("ledger")).unwrap();
    std::fs::write(
        scratch.path().join("ledger").join("preexisting.jsonl"),
        "{}\n",
    )
    .unwrap();

    let server = ProtocolServer::new();
    let result = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path,
            session_id: Some("preexisting".to_string()),
            initial_context: None,
        }))
        .await;

    let err = match result {
        Err(e) => e,
        Ok(_) => panic!(
            "protocol_start must reject a session_id whose ledger already exists, but it succeeded"
        ),
    };
    assert!(
        err.message.contains("preexisting") && err.message.contains("already exists"),
        "expected an 'already exists' error naming the session_id, got: {}",
        err.message
    );
}
