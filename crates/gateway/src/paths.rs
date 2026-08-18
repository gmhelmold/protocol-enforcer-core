// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! On-disk path resolution and opt-in confinement for the gateway
//! (ledger/sidecar locations, `manifest_path` jailing). Split out of
//! `server.rs` to keep that file under the size cap. Everything here is a
//! process/deployment concern resolved from the environment, not part of the
//! MCP tool surface.

use std::path::PathBuf;

use rmcp::ErrorData;

/// The on-disk ledger/sidecar directory. Reads `PROTOCOL_LEDGER_DIR` on
/// each call (default `./ledger`); resolved per-call rather than cached in a
/// `OnceLock` so that cwd-scoped integration tests (which `set_current_dir`
/// into a tempdir and rely on the relative `./ledger` default) stay
/// isolated, while a real deployment can point it at a durable absolute
/// path. Deliberately NOT profile-configurable: it is process/deployment
/// infrastructure, like `PROTOCOL_PROFILES_DIR`, not a per-run setting.
pub(crate) fn ledger_dir() -> PathBuf {
    std::env::var("PROTOCOL_LEDGER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./ledger"))
}

/// Base directory for the enforcer's artifact store (`<root>/.artifacts/...`).
/// Reads `PROTOCOL_ARTIFACT_DIR` (default `.`), resolved per-call like
/// `ledger_dir()`; deployment infra, not profile-configurable.
pub(crate) fn artifact_root() -> PathBuf {
    std::env::var("PROTOCOL_ARTIFACT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// The on-disk ledger path for `session_id`, matching `Ledger::new`'s
/// naming (`{ledger_dir}/{session_id}.jsonl`).
pub(crate) fn ledger_path(session_id: &str) -> PathBuf {
    ledger_dir().join(format!("{}.jsonl", session_id))
}

/// The on-disk sidecar path for `session_id`:
/// `<ledger_dir>/<session_id>.manifest.json`.
pub(crate) fn sidecar_path(session_id: &str) -> PathBuf {
    ledger_dir().join(format!("{}.manifest.json", session_id))
}

/// Atomically write the crash-recovery sidecar:
/// write to `<path>.tmp` then `rename` over the final path, so a reader
/// never observes a torn file. Best-effort: the caller logs and continues
/// on failure rather than failing `protocol_start` -- recovery is hardening,
/// not a start-time hard dependency.
pub(crate) fn write_sidecar_atomic(
    session_id: &str,
    manifest_path: &str,
    version: &str,
) -> std::io::Result<()> {
    std::fs::create_dir_all(ledger_dir())?;
    let final_path = sidecar_path(session_id);
    let tmp_path = final_path.with_extension("json.tmp");
    let contents = serde_json::json!({
        "manifest_path": manifest_path,
        "version": version,
    });
    std::fs::write(
        &tmp_path,
        serde_json::to_vec_pretty(&contents).unwrap_or_default(),
    )?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

/// Resolve a `protocol_start` manifest reference to a concrete YAML path,
/// returning `(path, was_name)`.
///
/// A bare profile NAME — a safe name (`protocol_manifest::is_safe_name`: no
/// path separators, no `..`) that does not already carry a `.yaml`/`.yml`
/// extension — resolves to `<profiles_dir>/<name>.yaml`, i.e. confined to the
/// served profiles tree BY CONSTRUCTION (the same name→file mapping the CLI
/// and introspection use). Anything else — a relative/absolute path, or a
/// reference that ends in `.yaml`/`.yml` — is treated as a filesystem path and
/// returned as-is; the caller still applies the `..` reject and opt-in
/// confinement to that branch. `was_name` lets the caller skip those
/// path-only guards for the already-confined name branch.
pub(crate) fn resolve_manifest_reference(reference: &str) -> (PathBuf, bool) {
    let lower = reference.to_ascii_lowercase();
    let looks_like_name = protocol_manifest::is_safe_name(reference)
        && !lower.ends_with(".yaml")
        && !lower.ends_with(".yml");
    if looks_like_name {
        (
            crate::introspect::profiles_dir().join(format!("{reference}.yaml")),
            true,
        )
    } else {
        (PathBuf::from(reference), false)
    }
}

/// Opt-in confinement root for `protocol_start`'s `manifest_path`. Returns
/// `Some` only when `PROTOCOL_PROFILES_DIR` is EXPLICITLY set — a deployer
/// declaring that profiles must be served only from that tree. When it is
/// unset the gateway keeps its co-trusted default (any `manifest_path`, with
/// only the cheap `..`-component reject applied by the caller). The root is
/// canonicalized so the later `starts_with` check is robust against symlinks
/// and `..`; a set-but-inaccessible dir is a hard configuration error.
pub(crate) fn manifest_confinement_root() -> Option<Result<PathBuf, ErrorData>> {
    let raw = std::env::var("PROTOCOL_PROFILES_DIR").ok()?;
    Some(std::fs::canonicalize(&raw).map_err(|e| {
        ErrorData::invalid_params(
            format!("PROTOCOL_PROFILES_DIR '{}' is not accessible: {}", raw, e),
            None,
        )
    }))
}

/// Resolve the path `protocol_start` will actually LOAD from a `manifest_path`.
/// For the name branch (`was_name`), the reference already resolved under
/// `PROTOCOL_PROFILES_DIR` by construction. For the path branch, opt-in
/// confinement applies: when `PROTOCOL_PROFILES_DIR` is set, the path is
/// canonicalized (resolving symlinks and any residual `.`/`..`) and must live
/// inside it, and the CANONICALIZED path is returned so a symlink swapped
/// between this check and the load cannot redirect outside the jail (TOCTOU).
/// Unconfined and name branches return the resolved path as-is.
pub(crate) fn resolve_load_path(manifest_path: &str, was_name: bool) -> Result<PathBuf, ErrorData> {
    if was_name {
        return Ok(PathBuf::from(manifest_path));
    }
    let Some(base) = manifest_confinement_root() else {
        return Ok(PathBuf::from(manifest_path));
    };
    let base = base?;
    let full = std::fs::canonicalize(manifest_path).map_err(|e| {
        ErrorData::invalid_params(format!("Failed to resolve manifest_path: {}", e), None)
    })?;
    if !full.starts_with(&base) {
        return Err(ErrorData::invalid_params(
            "manifest_path must be inside PROTOCOL_PROFILES_DIR".to_string(),
            None,
        ));
    }
    Ok(full)
}

/// Opt-in STRICT confinement for `protocol_start`'s `manifest_path`: reads
/// `PROTOCOL_MANIFEST_STRICT` on each call (truthy = `"1"` or `"true"`,
/// case-insensitive; unset or anything else = `false`), mirroring the
/// per-call env-read style of `ledger_dir()`/`artifact_root()`. When set, the
/// caller must reject any `manifest_path` that isn't a bare profile NAME
/// (`was_name == false` from `resolve_manifest_reference`) — the strongest,
/// confined-by-construction mode, for deployments that never want to accept
/// an arbitrary filesystem path from a client.
pub(crate) fn manifest_strict_mode() -> bool {
    std::env::var("PROTOCOL_MANIFEST_STRICT")
        .map(|v| v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::resolve_manifest_reference;

    /// The name-vs-path discriminator (`was_name`) is env-independent, so we can
    /// assert it directly. A bare safe name with no yaml extension is a NAME; a
    /// path separator, a `..`, an empty string, or a `.yaml`/`.yml` extension
    /// (in ANY case — the S1 fix) makes it a PATH.
    #[test]
    fn name_vs_path_discriminator() {
        for name in ["foo", "quick-bug-fix", "profile_v2"] {
            assert!(
                resolve_manifest_reference(name).1,
                "{name} should be a name"
            );
        }
        for path in [
            "foo.yaml",
            "foo.yml",
            "foo.YAML", // uppercase extension must NOT be treated as a name (S1)
            "foo.Yml",
            "PROFILE.YML",
            "../escape",
            "a/b",
            "abs/path.yaml",
            "",
        ] {
            assert!(
                !resolve_manifest_reference(path).1,
                "{path:?} should be a path, not a name"
            );
        }
    }
}
