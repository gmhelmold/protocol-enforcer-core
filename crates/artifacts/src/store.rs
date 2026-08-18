// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! ArtifactStore implementation with streaming SHA256

use mime_guess::from_path;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::fs::Permissions;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use protocol_types::{ArtifactError, ArtifactRef};

#[cfg(unix)]
const DIR_PERMISSIONS: u32 = 0o700;
#[cfg(unix)]
const FILE_PERMISSIONS: u32 = 0o600;

pub struct ArtifactStore {
    root: PathBuf,
    /// The root directory passed to `new`/`new_async`, i.e. the base against which
    /// `ArtifactRef.path` (a relative path) must be resolved to reach the real file.
    base_root: PathBuf,
}

/// Reject any path component that could escape the intended root: `..`
/// segments, and path separators (`/`, `\`) are the only way a caller-
/// controlled `step_id`/`ArtifactRef.path` can walk outside `root`/`base_root`
/// via a simple `Path::join`, since `join` treats a component containing a
/// separator as introducing new path segments and `..` as a parent hop.
/// Validates a relative path — a `step_id` or an `ArtifactRef.path` — that is
/// allowed to contain `/`-separated segments (the enforcer exposes the current
/// step as `macro_id/sub_state_id`, so a valid `step_id` legitimately contains a
/// `/`). Each segment still must not be `..`, empty, or itself contain a `\`, and
/// the path must not be absolute. Callers must additionally reject NUL bytes.
fn is_safe_relative_path(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    path.components().all(|c| match c {
        std::path::Component::Normal(seg) => {
            let seg = seg.to_string_lossy();
            !seg.is_empty() && !seg.contains('\\') && seg != ".."
        }
        std::path::Component::CurDir => true,
        _ => false,
    })
}

fn traversal_error(what: &str, value: &str) -> ArtifactError {
    ArtifactError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "invalid {}: path traversal or absolute path detected: {:?}",
            what, value
        ),
    ))
}

impl ArtifactStore {
    pub async fn new_async(session_id: &str, root: &Path) -> Result<Self, ArtifactError> {
        let artifact_root = root.join(".artifacts").join(session_id);
        fs::create_dir_all(&artifact_root).await?;
        Self::set_dir_permissions(&artifact_root).await?;
        Ok(Self {
            root: artifact_root,
            base_root: root.to_path_buf(),
        })
    }

    pub async fn store_async(
        &mut self,
        step_id: &str,
        data: &[u8],
    ) -> Result<ArtifactRef, ArtifactError> {
        let mime = from_path("artifact.bin")
            .first_or_octet_stream()
            .to_string();
        self.store_with_mime_async(step_id, data, &mime).await
    }

    pub async fn store_with_mime_async(
        &mut self,
        step_id: &str,
        data: &[u8],
        mime: &str,
    ) -> Result<ArtifactRef, ArtifactError> {
        if step_id.is_empty()
            || step_id.contains('\0')
            || !is_safe_relative_path(Path::new(step_id))
        {
            return Err(traversal_error("step_id", step_id));
        }
        let step_dir = self.root.join(step_id);
        fs::create_dir_all(&step_dir).await?;
        Self::set_dir_permissions(&step_dir).await?;

        let file_name = format!("{}.bin", Uuid::new_v4());
        let file_path = step_dir.join(&file_name);

        let (sha256, size) = Self::write_file_streaming_hash(&file_path, data).await?;

        let verified_sha256 = Self::verify_hash(&file_path, &sha256).await?;
        if verified_sha256 != sha256 {
            return Err(ArtifactError::HashMismatch {
                expected: sha256,
                actual: verified_sha256,
            });
        }

        // No post-write chmod: the file is created 0600 (see `create_restricted`),
        // so it is never world-readable even for an instant. The old
        // "create world-readable, chmod 0600 after write+verify" pattern was the
        // TOCTOU security-audit FIX 2 removes.

        // `file_path` is absolute (base_root/.artifacts/session_id/step_id/file_name).
        // Store the path relative to `base_root` so it can be resolved back to the
        // real file later (e.g. by `verify_async`), regardless of session_id nesting.
        let relative_path = file_path
            .strip_prefix(&self.base_root)
            .unwrap_or(&file_path)
            .to_path_buf();

        Ok(ArtifactRef {
            path: relative_path,
            sha256,
            size_bytes: size,
            mime_type: mime.to_string(),
        })
    }

    /// Verify that the file referenced by `artifact_ref.path` (resolved relative to
    /// `base_root`) still hashes to `artifact_ref.sha256`. The engine calls this instead
    /// of reading artifact content itself.
    pub async fn verify_async(&self, artifact_ref: &ArtifactRef) -> Result<bool, ArtifactError> {
        if !is_safe_relative_path(&artifact_ref.path) {
            return Err(traversal_error(
                "artifact_ref.path",
                &artifact_ref.path.to_string_lossy(),
            ));
        }
        let full_path = self.base_root.join(&artifact_ref.path);
        let mut file = fs::File::open(&full_path).await?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];

        loop {
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }

        let hash = format!("{:x}", hasher.finalize());
        Ok(hash == artifact_ref.sha256)
    }

    #[cfg(unix)]
    async fn set_dir_permissions(path: &Path) -> Result<(), ArtifactError> {
        let perms = Permissions::from_mode(DIR_PERMISSIONS);
        fs::set_permissions(path, perms).await?;
        Ok(())
    }

    #[cfg(not(unix))]
    async fn set_dir_permissions(_path: &Path) -> Result<(), ArtifactError> {
        // Unix-style mode bits don't map onto Windows ACLs; the platform's
        // default (user-owned, non-world-accessible under the user profile)
        // is the closest available equivalent, so this is a deliberate no-op.
        Ok(())
    }

    /// Create the artifact file already restricted to owner-only on unix
    /// (`0600`) so it is NEVER world-readable, not even for the window between
    /// creation and a later `chmod`. Closing that TOCTOU is the whole of
    /// security-audit FIX 2: an artifact can hold a signed approval or other
    /// sensitive bytes, and a co-tenant reader racing the old post-write chmod
    /// could have read them. On non-unix the platform default (user-profile
    /// ACLs) is the closest equivalent, so this is a plain `create`.
    #[cfg(unix)]
    async fn create_restricted(path: &Path) -> Result<fs::File, ArtifactError> {
        // `tokio::fs::OpenOptions::mode` is an inherent unix method (no std
        // `OpenOptionsExt` import needed). The mode is applied at open(2), so
        // the file never exists world-readable.
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(FILE_PERMISSIONS)
            .open(path)
            .await?;
        Ok(file)
    }

    #[cfg(not(unix))]
    async fn create_restricted(path: &Path) -> Result<fs::File, ArtifactError> {
        Ok(fs::File::create(path).await?)
    }

    async fn write_file_streaming_hash(
        path: &Path,
        data: &[u8],
    ) -> Result<(String, u64), ArtifactError> {
        let mut file = Self::create_restricted(path).await?;
        let mut hasher = Sha256::new();
        let mut total_written = 0u64;

        let mut reader = data;
        const CHUNK_SIZE: usize = 64 * 1024;
        let mut buffer = vec![0u8; CHUNK_SIZE];

        loop {
            let n = std::io::Read::read(&mut reader, &mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            file.write_all(&buffer[..n]).await?;
            total_written += n as u64;
        }

        file.flush().await?;
        drop(file);

        let hash = format!("{:x}", hasher.finalize());
        Ok((hash, total_written))
    }

    async fn verify_hash(path: &Path, _expected: &str) -> Result<String, ArtifactError> {
        let mut file = fs::File::open(path).await?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 64 * 1024];

        loop {
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }

        let hash = format!("{:x}", hasher.finalize());
        Ok(hash)
    }
}
