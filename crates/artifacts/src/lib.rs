// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Public API for protocol-artifacts crate

pub mod port;
pub mod store;

pub use crate::port::ArtifactPort;
use protocol_types::{ArtifactError, ArtifactRef};
use std::path::Path;

pub use store::ArtifactStore;

impl ArtifactStore {
    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        // V5-OK: Runtime creation failure indicates unrecoverable system error
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    pub fn new(session_id: &str, root: &Path) -> Result<Self, ArtifactError> {
        Self::block_on(async { Self::new_async(session_id, root).await })
    }

    pub fn store(&mut self, step_id: &str, data: &[u8]) -> Result<ArtifactRef, ArtifactError> {
        Self::block_on(async { self.store_async(step_id, data).await })
    }

    pub fn store_with_mime(
        &mut self,
        step_id: &str,
        data: &[u8],
        mime: &str,
    ) -> Result<ArtifactRef, ArtifactError> {
        Self::block_on(async { self.store_with_mime_async(step_id, data, mime).await })
    }

    pub fn verify(&self, artifact_ref: &ArtifactRef) -> Result<bool, ArtifactError> {
        Self::block_on(async { self.verify_async(artifact_ref).await })
    }
}

impl ArtifactPort for ArtifactStore {
    fn store(&mut self, step_id: &str, data: &[u8]) -> Result<ArtifactRef, ArtifactError> {
        self.store(step_id, data)
    }

    fn store_with_mime(
        &mut self,
        step_id: &str,
        data: &[u8],
        mime: &str,
    ) -> Result<ArtifactRef, ArtifactError> {
        self.store_with_mime(step_id, data, mime)
    }

    fn verify(&self, artifact_ref: &ArtifactRef) -> Result<bool, ArtifactError> {
        self.verify(artifact_ref)
    }
}
