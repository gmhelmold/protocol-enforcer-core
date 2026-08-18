//! Unit tests for ArtifactStore

use protocol_artifacts::port::ArtifactPort;
use protocol_artifacts::ArtifactStore;
use protocol_types::ArtifactRef;
use tempfile::tempdir;

#[test]
fn test_store_basic() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let data = b"hello world";
    let artifact = store.store("step-1", data).unwrap();

    assert_eq!(artifact.size_bytes, data.len() as u64);
    assert!(!artifact.sha256.is_empty());
    assert_eq!(artifact.mime_type, "application/octet-stream");
    assert!(artifact.path.to_string_lossy().contains("step-1"));
}

#[test]
fn test_store_with_mime() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let data = b"test";
    let artifact = store.store_with_mime("step-1", data, "text/plain").unwrap();

    assert_eq!(artifact.mime_type, "text/plain");
}

#[test]
fn test_store_large_data() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let data = vec![0u8; 1024 * 1024]; // 1MB
    let artifact = store.store("step-1", &data).unwrap();

    assert_eq!(artifact.size_bytes, data.len() as u64);
}

#[test]
fn test_multiple_artifacts_same_step() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();

    let a1 = store.store("step-1", b"first").unwrap();
    let a2 = store.store("step-1", b"second").unwrap();

    assert_ne!(a1.sha256, a2.sha256);
    assert_ne!(a1.path, a2.path);
}

#[test]
fn test_artifact_path_structure() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("session-123", dir.path()).unwrap();
    let artifact = store.store("step-abc", b"data").unwrap();

    let path_str = artifact.path.to_string_lossy();
    assert!(path_str.starts_with(".artifacts/"));
    assert!(path_str.contains("step-abc"));
    assert!(path_str.ends_with(".bin"));
}

#[test]
fn test_sha256_correctness() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let data = b"deterministic content";
    let artifact = store.store("step-1", data).unwrap();

    // Verify hash matches expected SHA256
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let expected = format!("{:x}", hasher.finalize());
    assert_eq!(artifact.sha256, expected);
}

#[test]
fn test_directory_permissions() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let _artifact = store.store("step-1", b"data").unwrap();

    let artifacts_dir = dir.path().join(".artifacts").join("test-session");
    let metadata = std::fs::metadata(&artifacts_dir).unwrap();
    let permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(permissions.mode() & 0o777, 0o700);
    }
}

#[test]
fn test_file_created_owner_only_no_world_read() {
    // Security-audit FIX 2: the artifact file must be owner-only (0600) from
    // creation — never world- or group-readable, not even briefly. Before the
    // fix the file was created with the umask default (typically 0644,
    // world-readable) and only chmod'd 0600 AFTER the full write+verify, a
    // TOCTOU window a co-tenant could read through.
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let artifact = store.store("step-1", b"sensitive").unwrap();

    // `artifact.path` is relative to the store root.
    let file_path = dir.path().join(&artifact.path);
    let metadata = std::fs::metadata(&file_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "artifact file must be 0600, got {mode:o}");
        assert_eq!(mode & 0o077, 0, "no group/other access permitted");
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
    }
}

#[test]
fn test_concurrent_stores() {
    let dir = tempdir().unwrap();

    let refs = std::thread::scope(|s| {
        let mut handles = vec![];
        for i in 0..10 {
            let dir_path = dir.path().to_path_buf();
            handles.push(s.spawn(move || {
                let mut store = ArtifactStore::new("test-session", &dir_path).unwrap();
                store
                    .store(&format!("step-{}", i), format!("data-{}", i).as_bytes())
                    .expect("concurrent store must succeed")
            }));
        }
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });

    // Read back what was persisted: each of the 10 distinct payloads must have
    // produced a distinct content hash AND a distinct on-disk path, at the right
    // size. A concurrency bug that clobbered or mixed up writes would collapse
    // one of these sets below 10 (or record the wrong size).
    let hashes: std::collections::BTreeSet<_> = refs.iter().map(|r| r.sha256.clone()).collect();
    let paths: std::collections::BTreeSet<_> = refs.iter().map(|r| r.path.clone()).collect();
    assert_eq!(
        hashes.len(),
        10,
        "distinct payloads must persist as distinct hashes"
    );
    assert_eq!(paths.len(), 10, "each artifact lands at its own path");
    for r in &refs {
        assert_eq!(r.size_bytes, 6, "each 'data-N' payload is 6 bytes: {r:?}");
    }
}

#[test]
fn test_empty_data() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let artifact = store.store("step-1", b"").unwrap();

    assert_eq!(artifact.size_bytes, 0);
    assert!(!artifact.sha256.is_empty());
}

#[test]
fn test_sync_api() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let artifact = store.store("step-1", b"sync test").unwrap();

    assert_eq!(artifact.size_bytes, 9);
    assert_eq!(artifact.mime_type, "application/octet-stream");
}

#[test]
fn test_sync_api_with_mime() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let artifact = store
        .store_with_mime("step-1", b"sync test", "application/json")
        .unwrap();

    assert_eq!(artifact.mime_type, "application/json");
}

#[test]
fn test_artifact_port_trait() {
    let dir = tempdir().unwrap();
    let mut store: Box<dyn ArtifactPort> =
        Box::new(ArtifactStore::new("test-session", dir.path()).unwrap());
    let artifact = store.store("step-1", b"trait test").unwrap();

    assert_eq!(artifact.size_bytes, 10);
}

#[test]
fn test_different_steps_different_dirs() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();

    let a1 = store.store("step-a", b"data a").unwrap();
    let a2 = store.store("step-b", b"data b").unwrap();

    assert!(a1.path.to_string_lossy().contains("step-a"));
    assert!(a2.path.to_string_lossy().contains("step-b"));
    assert_ne!(a1.path, a2.path);
}

#[test]
fn test_hash_verification_on_readback() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let data = b"verify me";
    let artifact = store.store("step-1", data).unwrap();

    // The internal verify_hash should have already validated
    // This test ensures no HashMismatch error was returned
    assert_eq!(artifact.size_bytes, data.len() as u64);
}

#[test]
fn test_mime_detection_fallback() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();
    let artifact = store.store("step-1", b"binary data").unwrap();

    // Default mime type should be application/octet-stream
    assert_eq!(artifact.mime_type, "application/octet-stream");
}

#[test]
fn test_override_mime_type() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();

    let artifact = store
        .store_with_mime("step-1", b"{}", "application/json")
        .unwrap();
    assert_eq!(artifact.mime_type, "application/json");

    let artifact = store
        .store_with_mime("step-1", b"text", "text/plain")
        .unwrap();
    assert_eq!(artifact.mime_type, "text/plain");
}

// ---------------------------------------------------------------------
// Path traversal rejection
// ---------------------------------------------------------------------

#[test]
fn test_store_rejects_traversal_step_id() {
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();

    let result = store.store("../../../etc", b"pwned");
    assert!(result.is_err(), "traversal step_id must be rejected");

    // Nothing must have been written outside the artifact root.
    assert!(!dir.path().join("etc").exists());
}

#[test]
fn test_store_accepts_composite_step_id_with_slash() {
    // Corrected contract: the enforcer exposes the current step as
    // `macro_id/sub_state_id` (profile_engine.rs), so a `/`-containing step_id is
    // legitimate and must be stored. Real traversal is still rejected — see
    // `test_store_rejects_traversal_step_id` and the async composite/traversal
    // regression test. (Previously this asserted rejection, which broke real
    // sessions that stored an artifact keyed by the current step.)
    let dir = tempdir().unwrap();
    let mut store = ArtifactStore::new("test-session", dir.path()).unwrap();

    let result = store.store("sub/dir", b"data");
    assert!(
        result.is_ok(),
        "composite macro/sub step_id must be accepted: {:?}",
        result.err()
    );
}

#[test]
fn test_verify_rejects_traversal_path() {
    let dir = tempdir().unwrap();
    let store = ArtifactStore::new("test-session", dir.path()).unwrap();

    let forged = ArtifactRef {
        path: std::path::PathBuf::from("../../../../etc/passwd"),
        sha256: "0".repeat(64),
        size_bytes: 0,
        mime_type: "text/plain".to_string(),
    };

    let result = store.verify(&forged);
    assert!(result.is_err(), "traversal artifact path must be rejected");
}

#[test]
fn test_verify_rejects_absolute_path() {
    let dir = tempdir().unwrap();
    let store = ArtifactStore::new("test-session", dir.path()).unwrap();

    let forged = ArtifactRef {
        path: std::path::PathBuf::from("/etc/passwd"),
        sha256: "0".repeat(64),
        size_bytes: 0,
        mime_type: "text/plain".to_string(),
    };

    let result = store.verify(&forged);
    assert!(result.is_err(), "absolute artifact path must be rejected");
}

// Async API tests
#[cfg(test)]
mod async_tests {
    use super::*;

    #[tokio::test]
    async fn test_store_async() {
        let dir = tempdir().unwrap();
        let mut store = ArtifactStore::new_async("test-session", dir.path())
            .await
            .unwrap();
        let data = b"hello world";
        let artifact = store.store_async("step-1", data).await.unwrap();

        assert_eq!(artifact.size_bytes, data.len() as u64);
        assert!(!artifact.sha256.is_empty());
        assert_eq!(artifact.mime_type, "application/octet-stream");
    }

    #[tokio::test]
    async fn test_store_with_mime_async() {
        let dir = tempdir().unwrap();
        let mut store = ArtifactStore::new_async("test-session", dir.path())
            .await
            .unwrap();
        let data = b"test";
        let artifact = store
            .store_with_mime_async("step-1", data, "text/plain")
            .await
            .unwrap();

        assert_eq!(artifact.mime_type, "text/plain");
    }

    #[tokio::test]
    async fn test_concurrent_async() {
        let dir = tempdir().unwrap();
        let mut handles = vec![];
        for i in 0..10 {
            let dir_path = dir.path().to_path_buf();
            handles.push(tokio::spawn(async move {
                let mut s = ArtifactStore::new_async("test-session", &dir_path)
                    .await
                    .unwrap();
                s.store_async(&format!("step-{}", i), format!("data-{}", i).as_bytes())
                    .await
            }));
        }

        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn composite_step_id_with_slash_is_accepted_but_traversal_rejected() {
        // Regression: the enforcer exposes the current step as `macro_id/sub_state_id`
        // (profile_engine.rs), so a legitimate step_id contains a `/`. The store must
        // accept it while still rejecting real traversal / absolute / NUL step_ids.
        let dir = tempdir().unwrap();
        let mut store = ArtifactStore::new_async("test-session", dir.path())
            .await
            .unwrap();

        let ok = store
            .store_with_mime_async("phase1/gate", b"data", "text/plain")
            .await;
        assert!(
            ok.is_ok(),
            "composite macro/sub step_id must store: {:?}",
            ok.err()
        );

        for bad in ["../escape", "phase1/../../etc", "/abs/path", "a\0b"] {
            let r = store.store_with_mime_async(bad, b"x", "text/plain").await;
            assert!(r.is_err(), "step_id {:?} must be rejected", bad);
        }
    }
}
