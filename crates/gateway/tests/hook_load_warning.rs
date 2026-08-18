// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Acceptance test for the gateway-side passivity advisory:
//! `protocol-gateway` never executes hooks (the passivity invariant), but a
//! hook-bearing profile served here used to load with no signal that its
//! hooks are inert. `protocol_start` must now log a `tracing::warn!` at
//! load time when the loaded profile declares hooks, and must NOT log one
//! when it doesn't -- and, either way, behavior (the served position) must
//! be byte-identical, since the passivity invariant forbids the warning
//! from changing anything about how the profile is handled.

use std::sync::{Arc, Mutex};

use rmcp::handler::server::wrapper::Parameters;
use tracing_subscriber::fmt::MakeWriter;

use protocol_gateway::server::{ProtocolServer, ProtocolStartRequest};

/// Two macros: `draft` (non-final: inject `s1`, checklist `chk1`) and `ship`
/// (final: checklist `chk2`). `with_hooks` optionally attaches an inline
/// hook to `s1` and macro `draft`.
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
name: "hook-warning-test"
version: "1.0.0"
description: "gateway hook-warning acceptance test profile"

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

/// Serializes cwd changes AND the global tracing subscriber's shared log
/// buffer across the two tests in this binary (mirroring
/// `crash_recovery.rs`'s `CWD_LOCK`/`ScratchCwd` pattern).
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuf {
    type Writer = SharedBuf;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

struct ScratchCwd {
    original: std::path::PathBuf,
    tempdir: tempfile::TempDir,
    buf: SharedBuf,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl ScratchCwd {
    /// Enters a scratch cwd AND installs a process-global tracing
    /// subscriber (safe: only one is ever live at a time, serialized by
    /// `LOCK`) writing to a fresh, empty buffer this call owns.
    async fn enter() -> Self {
        let guard = LOCK.lock().await;
        let original = std::env::current_dir().unwrap();
        let tempdir = tempfile::TempDir::new().unwrap();
        std::env::set_current_dir(tempdir.path()).unwrap();

        let buf = SharedBuf::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .without_time()
            .finish();
        // `set_global_default` errors if already set (e.g. by a prior test
        // in this binary); that's fine -- the buffer is per-call, but a
        // stale global subscriber would keep writing to the FIRST call's
        // buffer, not this one, so tests here run one at a time (via
        // `LOCK`) rather than relying on a fresh subscriber each time.
        let _ = tracing::subscriber::set_global_default(subscriber);

        Self {
            original,
            tempdir,
            buf,
            _guard: guard,
        }
    }

    fn path(&self) -> &std::path::Path {
        self.tempdir.path()
    }

    fn log(&self) -> String {
        String::from_utf8_lossy(&self.buf.0.lock().unwrap()).into_owned()
    }

    fn clear_log(&self) {
        self.buf.0.lock().unwrap().clear();
    }
}

impl Drop for ScratchCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

// Both the has-hooks and no-hooks cases live in ONE test on purpose. The
// warning is emitted through a process-GLOBAL tracing subscriber (the gateway
// warns from inside a `spawn_blocking` closure, a different thread, so a
// thread-local subscriber would not capture it). `set_global_default` binds
// once per process, so two separate tests would leave the second one reading a
// buffer the subscriber never writes to — deterministically empty, and which
// case that hits depends on test execution order. Driving both cases through
// the single subscriber+buffer, sequentially with a clear in between, removes
// the ordering dependency entirely.
#[tokio::test]
async fn warns_only_when_profile_has_hooks() {
    let scratch = ScratchCwd::enter().await;
    let server = ProtocolServer::new();

    // Case 1: a profile that declares hooks must log the passive-gateway warning.
    let hooked = write_profile(scratch.path(), true);
    let start = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: hooked,
            session_id: Some("hook-warn-session".to_string()),
            initial_context: None,
        }))
        .await
        .expect("protocol_start must succeed even with hooks (passivity: they never run)");
    // Passivity: served position is identical to the no-hooks case.
    assert_eq!(start.0["position"]["sub_state_id"], "s1");
    let log = scratch.log();
    assert!(
        log.contains("hook") && (log.contains("passive") || log.contains("never")),
        "expected a passive-gateway hook warning in the log, got: {log}"
    );

    // Case 2: a profile with no hooks must not warn about hooks.
    scratch.clear_log();
    let hookless = write_profile(scratch.path(), false);
    let start = server
        .protocol_start(Parameters(ProtocolStartRequest {
            manifest_path: hookless,
            session_id: Some("no-hook-session".to_string()),
            initial_context: None,
        }))
        .await
        .expect("protocol_start must succeed");
    assert_eq!(start.0["position"]["sub_state_id"], "s1");
    let log = scratch.log();
    assert!(
        !log.contains("hook"),
        "no hooks declared -- must not warn about hooks, got: {log}"
    );
}
