// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! ArtifactPort trait definition

use protocol_types::{ArtifactError, ArtifactRef};

pub trait ArtifactPort: Send + Sync {
    fn store(&mut self, step_id: &str, data: &[u8]) -> Result<ArtifactRef, ArtifactError>;
    fn store_with_mime(
        &mut self,
        step_id: &str,
        data: &[u8],
        mime: &str,
    ) -> Result<ArtifactRef, ArtifactError>;
    /// Verify that the artifact's on-disk content still hashes to `artifact_ref.sha256`.
    /// Returns Ok(true) on match, Ok(false) on mismatch, Err on IO failure (e.g. missing file).
    fn verify(&self, artifact_ref: &ArtifactRef) -> Result<bool, ArtifactError>;
}
