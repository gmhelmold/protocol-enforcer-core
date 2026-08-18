// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Tier-1 MCP server surface for the Protocol Enforcer.
//!
//! Exposes the driving tools — `protocol_start`,
//! `protocol_submit_milestone`, `protocol_get_state`, and `protocol_store_artifact`
//! (the enforcer-authored artifact offload + integrity gate) —
//! plus the two read-only introspection tools
//! (`protocol_profile_list`/`protocol_profile_show`, see `introspect.rs`), for 6 total.
//!
//! Drives `protocol_fsm::ProfileFsmEngine` (the nested macro/sub-state
//! model). Session state is kept correct across concurrent
//! sessions by holding a registry of one `ProfileFsmEngine` per
//! `session_id`, EACH behind its own [`std::sync::Mutex`] so concurrent calls
//! that name the SAME session serialize while different sessions run in
//! parallel — the engine is never removed from the registry mid-call, so there
//! is no window in which a second concurrent call recovers a stale copy from
//! the ledger. This gateway RESOLVES NOTHING itself: rendering
//! (`RenderedInjection`) is already done by the engine/library; the
//! gateway only drives `start_session`/`submit_milestone`/`get_state` and
//! serializes their results into the MCP wire shapes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;

use protocol_fsm::{FsmConfig, ProfileFsmEngine, StepOutcome};
use protocol_ledger::Ledger;
use protocol_library::Library;
use protocol_types::Position;

use crate::error_map::{fsm_error_to_mcp, manifest_invalid_error};
use crate::paths::{
    artifact_root, ledger_dir, ledger_path, resolve_load_path, write_sidecar_atomic,
};
use crate::wire::{
    advance_response_json, completed_response_json, looped_back_response_json, profile_sha256_of,
    session_not_found_error, session_status_str, start_response_json, transcript_root_best_effort,
};

const LEDGER_TAIL_LEN: usize = 20;

/// One session's engine behind its own lock. A `std::sync::Mutex` (not tokio's)
/// so it can be held across the synchronous `submit_milestone`/`get_state`
/// calls inside `spawn_blocking`; the registry only ever hands out a clone of
/// this `Arc`, never removes the engine, so same-session calls serialize here
/// while different sessions proceed independently.
type SharedEngine = Arc<std::sync::Mutex<ProfileFsmEngine<Ledger>>>;

/// Lock a [`SharedEngine`], recovering the guard if a previous holder panicked
/// (a poisoned lock must not wedge every later call to that session).
fn lock_engine(engine: &SharedEngine) -> std::sync::MutexGuard<'_, ProfileFsmEngine<Ledger>> {
    engine
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProtocolStartRequest {
    /// The profile to drive this session: either a bare profile NAME
    /// (resolved to `<PROTOCOL_PROFILES_DIR>/<name>.yaml`, like the CLI and
    /// introspection) or a path to a profile YAML file.
    pub manifest_path: String,
    /// Optional caller-supplied session id. A UUIDv4 is generated if omitted.
    pub session_id: Option<String>,
    /// Optional initial context seeded into the first sub-state's rendered context.
    pub initial_context: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProtocolSubmitMilestoneRequest {
    /// Session id returned by `protocol_start`.
    pub session_id: String,
    /// The macro-state id the caller believes is currently active.
    pub macro_id: String,
    /// The sub-state id the caller believes is currently active.
    pub sub_state_id: String,
    /// Evidence for each item in the current sub-state's `criteria` (checklist),
    /// or `{"approval_signature": "<hex>"}` on a `human_approval` gate. Ignored
    /// on every other sub-state type.
    pub evidence: Option<HashMap<String, serde_json::Value>>,
    /// The final payload, required only on the final macro-state's checklist
    /// (validated against `Profile.output_contract`, if any).
    pub output: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProtocolGetStateRequest {
    /// Session id returned by `protocol_start`.
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProtocolProfileShowRequest {
    /// The profile name to inspect (path-safe; no `../`, `/`, or `\`).
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProtocolStoreArtifactRequest {
    /// Session id returned by `protocol_start`.
    pub session_id: String,
    /// A label for the step producing this artifact (path-safe component).
    pub step_id: String,
    /// The artifact content (UTF-8 text; the enforcer stores its bytes and
    /// computes the sha256). Binary via base64 is a future extension.
    pub content: String,
    /// MIME type; defaults to `text/plain` when omitted.
    pub mime_type: Option<String>,
}

/// Maximum length allowed for a client-supplied `session_id`.
const MAX_SESSION_ID_LEN: usize = 128;

/// Validate a client-supplied `session_id` against a strict allow-list
/// before it is ever used to build a filesystem path (`Ledger::new`/
/// `Ledger::replay`'s `dir.join(format!("{session_id}.jsonl"))`, and the
/// output-contract `destination.replace("{session_id}", session_id)` +
/// `std::fs::write`). Without this, a value like `"../../tmp/x"` escapes
/// the intended ledger/artifact directories (path traversal -> arbitrary
/// file write). The auto-generated (`None`) case always produces a UUIDv4
/// and is not passed through this check.
fn validate_session_id(session_id: &str) -> Result<(), ErrorData> {
    if session_id.is_empty() {
        return Err(ErrorData::invalid_params(
            "Invalid session_id: must not be empty".to_string(),
            None,
        ));
    }
    if session_id.len() > MAX_SESSION_ID_LEN {
        return Err(ErrorData::invalid_params(
            format!(
                "Invalid session_id: must be at most {} characters",
                MAX_SESSION_ID_LEN
            ),
            None,
        ));
    }
    if !session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ErrorData::invalid_params(
            "Invalid session_id: only ASCII letters, digits, '_' and '-' are allowed".to_string(),
            None,
        ));
    }
    Ok(())
}

/// Passive MCP gateway: a registry of one `ProfileFsmEngine` per active
/// session, driven entirely by the external MCP client over the 3 tools
/// below.
#[derive(Clone)]
pub struct ProtocolServer {
    sessions: Arc<Mutex<HashMap<String, SharedEngine>>>,
    tool_router: ToolRouter<Self>,
}

impl ProtocolServer {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            tool_router: Self::tool_router(),
        }
    }

    /// The exact set of model-facing MCP tool NAMES this server exposes — the
    /// registry the model can `list`/`invoke`. Read straight off the same
    /// `tool_router` the MCP `list_tools` capability serves, so it is the model's
    /// real tool surface, not a hand-kept copy. Used to assert that a verify
    /// hook (`verify-fact` or similar) is NEVER a callable tool: it runs only
    /// driver-side, off the model's reach.
    pub fn model_facing_tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    /// Return the session's [`SharedEngine`] handle: a cache hit clones the
    /// `Arc` out of the registry (releasing the map lock immediately), and a
    /// cache miss rebuilds the engine from its on-disk ledger + sidecar
    /// and installs it. Recovery is double-checked: a
    /// concurrent caller that recovered first wins the `entry`, and this call's
    /// own rebuilt engine is dropped — so a session is represented by exactly
    /// one `Arc<Mutex<..>>` and same-session calls serialize on it. A session
    /// with no on-disk state still surfaces `SESSION_NOT_FOUND`.
    async fn get_or_recover(&self, session_id: &str) -> Result<SharedEngine, ErrorData> {
        {
            let sessions = self.sessions.lock().await;
            if let Some(engine) = sessions.get(session_id) {
                return Ok(engine.clone());
            }
        }
        // Cache miss: rebuild off-lock (the blocking load never holds the map
        // mutex), then install under a fresh lock, preferring any engine a
        // concurrent caller already installed.
        let sid = session_id.to_string();
        let recovered =
            tokio::task::spawn_blocking(move || crate::recovery::recover_into_cache(&sid))
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("recovery task panicked: {}", e), None)
                })??;
        let mut sessions = self.sessions.lock().await;
        let engine = sessions
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(std::sync::Mutex::new(recovered)))
            .clone();
        Ok(engine)
    }
}

impl Default for ProtocolServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl ProtocolServer {
    #[tool(
        description = "Start a new Protocol Enforcer session from a nested profile YAML. Returns { session_id, position{macro_id,sub_state_id}, injected, macro{state_id,name}, session_state }.",
        output_schema = crate::wire::object_output_schema(),
    )]
    pub async fn protocol_start(
        &self,
        Parameters(req): Parameters<ProtocolStartRequest>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        // `manifest_path` is a bare profile NAME or a filesystem path (resolved
        // just below). A `..` component lets a path caller climb out of the
        // intended tree — reject relative traversal fail-closed. This `..`
        // reject is the always-on cheap guard for the PATH branch (a bare name
        // never contains `..`, so it is unaffected). Full confinement to
        // PROTOCOL_PROFILES_DIR is additionally applied OPT-IN inside the
        // spawn_blocking closure below for the path branch, only when that env
        // var is explicitly set; unset keeps the co-trusted arbitrary-path
        // default. The name branch is confined to that tree BY CONSTRUCTION.
        if Path::new(&req.manifest_path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ErrorData::invalid_params(
                "manifest_path must not contain '..' path components".to_string(),
                None,
            ));
        }

        if let Some(candidate) = req.session_id.as_deref() {
            validate_session_id(candidate)?;
        }

        let session_id = req
            .session_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        {
            let sessions = self.sessions.lock().await;
            if sessions.contains_key(&session_id) {
                return Err(ErrorData::invalid_params(
                    format!("Session '{}' already exists", session_id),
                    None,
                ));
            }
        }

        // Resolve the reference: a bare profile NAME resolves under
        // PROTOCOL_PROFILES_DIR (confined by construction); anything else is a
        // filesystem path (co-trusted, guarded by the `..` reject above and the
        // opt-in confinement inside the closure). The RESOLVED path is what we
        // load AND what we persist in the crash-recovery sidecar, so a later
        // recovery reloads the same file rather than a bare name.
        let (resolved_path, was_name) =
            crate::paths::resolve_manifest_reference(&req.manifest_path);

        // Opt-in STRICT mode (PROTOCOL_MANIFEST_STRICT): reject any
        // manifest_path that isn't a bare profile NAME, before any load and
        // before the ledger claim below.
        if !was_name && crate::paths::manifest_strict_mode() {
            return Err(ErrorData::invalid_params(
                "manifest_path must be a bare profile name when PROTOCOL_MANIFEST_STRICT is set"
                    .to_string(),
                None,
            ));
        }

        let manifest_path = resolved_path.to_string_lossy().into_owned();
        let initial_context = req.initial_context.clone();
        let sid = session_id.clone();

        // Atomically CLAIM the ledger file for this session_id, closing the
        // TOCTOU window a plain `exists()` check leaves open: two concurrent
        // (possibly cross-process) `protocol_start` calls for the same
        // session_id could both observe "no file" and both proceed to
        // `Ledger::new` (create+append) the same path, each appending its own
        // `SessionStarted` (bug C2). `create_new` makes the OS itself the
        // single arbiter of who wins. `session_id` was already validated
        // against the strict allow-list above, so this join is safe.
        //
        // This claim creates an EMPTY ledger file (`Ledger::new` below
        // reattaches to it in append mode). If ANYTHING after a successful
        // claim fails before the session is fully started and inserted, the
        // empty claim file MUST be removed -- see the cleanup on every error
        // path below -- so a corrected retry with the same session_id is not
        // permanently bricked. It is never removed on the success path.
        std::fs::create_dir_all(ledger_dir()).map_err(|e| {
            ErrorData::internal_error(format!("Failed to create ledger dir: {}", e), None)
        })?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(ledger_path(&session_id))
        {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(ErrorData::invalid_params(
                    format!("Session '{}' already exists (ledger present)", session_id),
                    None,
                ));
            }
            Err(e) => {
                return Err(ErrorData::internal_error(
                    format!("Failed to claim ledger file: {}", e),
                    None,
                ));
            }
        }

        // Profile loading, Ledger construction, validation, and the FSM's
        // synchronous `start_session` all touch blocking I/O. Run the whole
        // synchronous sequence on a blocking thread, mirroring the flat
        // server's pattern.
        let spawn_result = tokio::task::spawn_blocking(move || -> Result<_, ErrorData> {
            // Resolve (and, when PROTOCOL_PROFILES_DIR is set, confine) the path
            // we actually load; see `paths::resolve_load_path` for the TOCTOU-safe
            // canonicalization. The LOADED path is what the profile hash covers.
            let load_path = resolve_load_path(&manifest_path, was_name)?;
            let profile = protocol_manifest::load_profile(&load_path).map_err(|e| {
                ErrorData::invalid_params(format!("Failed to load profile: {}", e), None)
            })?;
            let library = Library::from_env();
            if let Err(violations) = protocol_manifest::validate_profile(&profile, &library) {
                return Err(manifest_invalid_error(violations));
            }
            let version = profile.version.clone();
            let ledger = Ledger::new(&sid, &ledger_dir()).map_err(|e| {
                ErrorData::internal_error(format!("Ledger init failed: {}", e), None)
            })?;
            let mut config = FsmConfig::from_settings(&profile.settings);
            config.artifact_root = artifact_root();
            // Hash the bytes of the file we actually loaded (canonicalized when
            // confined) so the transcript root commits to the protocol in force (§5).
            config.profile_sha256 = profile_sha256_of(&load_path);
            let mut engine = ProfileFsmEngine::new(profile, library, ledger, config);
            let view = engine
                .start_session(&sid, initial_context)
                .map_err(fsm_error_to_mcp)?;
            // Persist the path we actually loaded (canonicalized when
            // confined) so crash recovery reloads the exact same file.
            let sidecar_manifest = load_path.to_string_lossy().into_owned();
            Ok((engine, view, version, sidecar_manifest))
        })
        .await
        .map_err(|e| {
            ErrorData::internal_error(format!("protocol_start task panicked: {}", e), None)
        });

        // Everything below runs after we own the ledger claim above. Any
        // error from here on must clean up the empty claimed file (see the
        // comment on the claim) before propagating -- otherwise a corrected
        // retry with the same session_id would be permanently bricked by a
        // leftover empty ledger.
        let (engine, view, profile_version, sidecar_manifest_path) =
            match spawn_result.and_then(|inner| inner) {
                Ok(v) => v,
                Err(e) => {
                    let _ = std::fs::remove_file(ledger_path(&session_id));
                    return Err(e);
                }
            };

        let mut sessions = self.sessions.lock().await;
        if sessions.contains_key(&session_id) {
            drop(sessions);
            let _ = std::fs::remove_file(ledger_path(&session_id));
            return Err(ErrorData::invalid_params(
                format!("Session '{}' already exists", session_id),
                None,
            ));
        }
        sessions.insert(session_id.clone(), Arc::new(std::sync::Mutex::new(engine)));
        drop(sessions);

        // Best-effort crash-recovery sidecar: a
        // failure here must NOT fail `protocol_start` -- recovery is
        // hardening, not a start-time hard dependency.
        if let Err(e) = write_sidecar_atomic(&session_id, &sidecar_manifest_path, &profile_version)
        {
            tracing::warn!(
                session_id = %session_id,
                error = %e,
                "failed to write crash-recovery sidecar"
            );
        }

        Ok(Json(start_response_json(&session_id, &view)))
    }

    #[tool(
        description = "Submit milestone evidence for the current macro/sub-state. Returns { advanced: true, position, injected, macro, session_state, contract_written? } on success, { advanced: false, rejected_items, reason, position } on checklist rejection, or { advanced: false, looped_back: true, rejected_items, position, injected, macro, session_state } when the rejection loop-backed to the macro's first execute sub-state.",
        output_schema = crate::wire::object_output_schema(),
    )]
    pub async fn protocol_submit_milestone(
        &self,
        Parameters(req): Parameters<ProtocolSubmitMilestoneRequest>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        // Get the session's own-locked engine (recovering from disk on a cache
        // miss). The engine STAYS in the registry; the
        // per-session lock (taken inside the blocking closure below) serializes
        // concurrent same-session submissions, so a second call can never
        // recover a stale copy while this one is mid-flight.
        let engine = self.get_or_recover(&req.session_id).await?;

        let session_id = req.session_id.clone();
        let position = Position {
            macro_id: req.macro_id.clone(),
            sub_state_id: req.sub_state_id.clone(),
        };
        let evidence = req.evidence.clone().unwrap_or_default();
        let output = req.output.clone();

        // `submit_milestone` is synchronous and, on the final checklist,
        // writes the output-contract payload to disk -- run it on a blocking
        // thread. The per-session lock is taken INSIDE the blocking closure and
        // held only for the duration of the call; the engine is never removed
        // from the registry, so it always survives for subsequent calls and a
        // concurrent same-session call simply waits here.
        let outcome = tokio::task::spawn_blocking(move || {
            lock_engine(&engine).submit_milestone(&session_id, position, evidence, output)
        })
        .await
        .map_err(|e| {
            ErrorData::internal_error(format!("submit_milestone task panicked: {}", e), None)
        })?;

        let outcome = outcome.map_err(fsm_error_to_mcp)?;

        let response = match outcome {
            StepOutcome::Advanced(view) => advance_response_json(&view),
            StepOutcome::Rejected {
                rejected_items,
                reason,
                position,
            } => serde_json::json!({
                "advanced": false,
                "rejected_items": rejected_items,
                "reason": reason,
                "position": position,
            }),
            StepOutcome::LoopedBack {
                rejected_items,
                view,
            } => looped_back_response_json(&view, &rejected_items),
            StepOutcome::Completed { contract_path } => {
                // The engine appended `SessionCompleted` before returning, so
                // the ledger is final; compute the transcript root over it and
                // hand it to the caller (§6). Best-effort — omitted on failure.
                let transcript_root = transcript_root_best_effort(&req.session_id);
                completed_response_json(contract_path, transcript_root)
            }
        };

        Ok(Json(response))
    }

    #[tool(
        description = "Get the current state of a session. Returns { session_state, position?, ledger_tail }.",
        output_schema = crate::wire::object_output_schema(),
    )]
    pub async fn protocol_get_state(
        &self,
        Parameters(req): Parameters<ProtocolGetStateRequest>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        // Get the session's own-locked engine (recovering from disk on a cache
        // miss). Snapshot the `SessionState` under the
        // per-session lock (a clone, so the guard is released before we replay
        // the ledger tail); the engine is never removed from the registry.
        let engine = self.get_or_recover(&req.session_id).await?;
        let session_id = req.session_id.clone();
        let state = tokio::task::spawn_blocking(move || {
            lock_engine(&engine).get_state(&session_id).cloned()
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("get_state task panicked: {}", e), None))?
        .ok_or_else(|| session_not_found_error(&req.session_id))?;

        Self::get_state_response(&req.session_id, &state)
    }

    /// Builds the `protocol_get_state` response, replaying the
    /// ledger tail. Split out so both the cache-hit and post-recovery paths
    /// share it.
    fn get_state_response(
        session_id: &str,
        state: &protocol_types::SessionState,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        let ledger_events = Ledger::replay(session_id, &ledger_dir())
            .map_err(|e| ErrorData::internal_error(format!("Ledger replay failed: {}", e), None))?;
        let tail_start = ledger_events.len().saturating_sub(LEDGER_TAIL_LEN);
        let ledger_tail = &ledger_events[tail_start..];

        let response = serde_json::json!({
            "session_state": session_status_str(&state.status),
            "position": state.position,
            "ledger_tail": ledger_tail,
        });

        Ok(Json(response))
    }

    #[tool(
        description = "Offload an artifact THROUGH the enforcer's own store: it writes the bytes, computes an enforcer-authored sha256, tracks the ref on the session (ledgered, so it survives recovery), and re-verifies every tracked artifact's integrity before the session may complete. Returns { path, sha256, size_bytes, mime_type }.",
        output_schema = crate::wire::object_output_schema(),
    )]
    pub async fn protocol_store_artifact(
        &self,
        Parameters(req): Parameters<ProtocolStoreArtifactRequest>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        // Recover-on-miss like the other session-scoped tools; the engine stays
        // in the registry and the per-session lock serializes with concurrent
        // submits/stores.
        let engine = self.get_or_recover(&req.session_id).await?;
        let session_id = req.session_id.clone();
        let step_id = req.step_id.clone();
        let data = req.content.into_bytes();
        let mime = req.mime_type.unwrap_or_else(|| "text/plain".to_string());

        let artifact_ref = tokio::task::spawn_blocking(move || {
            lock_engine(&engine).store_artifact(&session_id, &step_id, &data, &mime)
        })
        .await
        .map_err(|e| {
            ErrorData::internal_error(format!("store_artifact task panicked: {}", e), None)
        })?
        .map_err(fsm_error_to_mcp)?;

        Ok(Json(serde_json::json!({
            "path": artifact_ref.path,
            "sha256": artifact_ref.sha256,
            "size_bytes": artifact_ref.size_bytes,
            "mime_type": artifact_ref.mime_type,
        })))
    }

    #[tool(
        description = "List the profiles this gateway can serve (read-only). Returns { profiles: [name, ...] }.",
        output_schema = crate::wire::object_output_schema(),
    )]
    pub async fn protocol_profile_list(&self) -> Result<Json<serde_json::Value>, ErrorData> {
        crate::introspect::list().map(Json)
    }

    #[tool(
        description = "Show a profile's structure (read-only, hook-blind): pipeline of macros -> sub-states with ids/names/types/criteria. Returns the structural view; never mutates.",
        output_schema = crate::wire::object_output_schema(),
    )]
    pub async fn protocol_profile_show(
        &self,
        Parameters(req): Parameters<ProtocolProfileShowRequest>,
    ) -> Result<Json<serde_json::Value>, ErrorData> {
        crate::introspect::show(&req.name).map(Json)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ProtocolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Protocol Enforcer MCP Gateway. Drive a pipeline session with: \
                 protocol_start(manifest_path, session_id?, initial_context?), \
                 protocol_submit_milestone(session_id, macro_id, sub_state_id, evidence?, output?), \
                 protocol_get_state(session_id).",
            )
    }
}
