use chrono::{DateTime, TimeZone, Utc};
use protocol_types::{
    ArtifactError, ArtifactRef, ChecklistEvidence, CircuitBreakerType, ContractError, FsmError,
    FsmEvent, FsmEventType, GatewayError, LedgerError, ManifestError, ProtocolStartParams,
    ProtocolSubmitParams, ProtocolSubmitResult, StepInfo, ValidationError,
};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

fn sample_datetime() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0)
        .single()
        .unwrap()
}

fn sample_uuid() -> Uuid {
    Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
}

#[test]
fn fsm_event_roundtrip() {
    let event = FsmEvent {
        event_id: sample_uuid(),
        timestamp: sample_datetime(),
        session_id: "session-123".to_string(),
        event_type: FsmEventType::SessionStarted {
            pipeline_id: "pipeline-1".to_string(),
            manifest_version: "1.0.0".to_string(),
            profile_sha256: None,
        },
        step_id: Some("step-1".to_string()),
        payload: serde_json::json!({"key": "value"}),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: FsmEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event.event_id, deserialized.event_id);
    assert_eq!(event.timestamp, deserialized.timestamp);
    assert_eq!(event.session_id, deserialized.session_id);
    assert_eq!(event.step_id, deserialized.step_id);
    assert_eq!(event.payload, deserialized.payload);
}

#[test]
fn fsm_event_type_all_variants_roundtrip() {
    let variants = vec![
        FsmEventType::SessionStarted {
            pipeline_id: "pipeline-1".to_string(),
            manifest_version: "1.0.0".to_string(),
            profile_sha256: None,
        },
        FsmEventType::MilestoneSubmitted {
            evidence_keys: vec!["item1".to_string(), "item2".to_string()],
            iteration: 1,
        },
        FsmEventType::MilestoneAccepted {
            next_step_id: Some("step-2".to_string()),
        },
        FsmEventType::MilestoneRejected {
            rejected_items: vec!["item1".to_string()],
            reason: "missing evidence".to_string(),
            iteration: 2,
        },
        FsmEventType::StepAdvanced {
            from_step: "step-1".to_string(),
            to_step: "step-2".to_string(),
        },
        FsmEventType::CircuitBreakerTriggered {
            breaker: CircuitBreakerType::MaxIterations,
            details: serde_json::json!({"max": 3}),
        },
        FsmEventType::SessionCompleted {
            final_artifact_path: "/path/to/artifact.json".to_string(),
            output_sha256: None,
        },
        FsmEventType::SessionFailed {
            reason: "timeout".to_string(),
            final_step: "step-3".to_string(),
        },
    ];

    for variant in variants {
        let json = serde_json::to_string(&variant).unwrap();
        let deserialized: FsmEventType = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{:?}", variant), format!("{:?}", deserialized));
    }
}

#[test]
fn circuit_breaker_type_roundtrip() {
    let variants = vec![
        CircuitBreakerType::MaxIterations,
        CircuitBreakerType::RepetitiveLoop,
        CircuitBreakerType::GlobalTimeout,
        CircuitBreakerType::ApprovalRejectionLimit,
    ];

    for variant in variants {
        let json = serde_json::to_string(&variant).unwrap();
        let deserialized: CircuitBreakerType = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{:?}", variant), format!("{:?}", deserialized));
    }
}

#[test]
fn checklist_evidence_roundtrip() {
    let mut evidence: ChecklistEvidence = HashMap::new();
    evidence.insert("item1".to_string(), serde_json::json!("value1"));
    evidence.insert("item2".to_string(), serde_json::json!({"nested": true}));

    let json = serde_json::to_string(&evidence).unwrap();
    let deserialized: ChecklistEvidence = serde_json::from_str(&json).unwrap();
    assert_eq!(evidence, deserialized);
}

#[test]
fn step_info_roundtrip() {
    let step = StepInfo {
        step_id: "step-1".to_string(),
        name: "First Step".to_string(),
        exit_checklist: vec!["item1".to_string(), "item2".to_string()],
    };

    let json = serde_json::to_string(&step).unwrap();
    let deserialized: StepInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(step.step_id, deserialized.step_id);
    assert_eq!(step.name, deserialized.name);
    assert_eq!(step.exit_checklist, deserialized.exit_checklist);
}

#[test]
fn artifact_ref_roundtrip() {
    let artifact = ArtifactRef {
        path: PathBuf::from("/artifacts/session-123/step-1/abc123.bin"),
        sha256: "abc123def456".to_string(),
        size_bytes: 1024,
        mime_type: "application/json".to_string(),
    };

    let json = serde_json::to_string(&artifact).unwrap();
    let deserialized: ArtifactRef = serde_json::from_str(&json).unwrap();
    assert_eq!(artifact.path, deserialized.path);
    assert_eq!(artifact.sha256, deserialized.sha256);
    assert_eq!(artifact.size_bytes, deserialized.size_bytes);
    assert_eq!(artifact.mime_type, deserialized.mime_type);
}

#[test]
fn protocol_start_params_roundtrip() {
    let params = ProtocolStartParams {
        manifest_path: "/path/to/manifest.yaml".to_string(),
        session_id: Some("session-123".to_string()),
        initial_context: Some(serde_json::json!({"key": "value"})),
    };

    let json = serde_json::to_string(&params).unwrap();
    let deserialized: ProtocolStartParams = serde_json::from_str(&json).unwrap();
    assert_eq!(params.manifest_path, deserialized.manifest_path);
    assert_eq!(params.session_id, deserialized.session_id);
    assert_eq!(params.initial_context, deserialized.initial_context);
}

#[test]
fn protocol_submit_params_roundtrip() {
    let mut evidence: ChecklistEvidence = HashMap::new();
    evidence.insert("item1".to_string(), serde_json::json!("done"));

    let mut artifacts = HashMap::new();
    artifacts.insert(
        "output".to_string(),
        ArtifactRef {
            path: PathBuf::from("/artifacts/output.json"),
            sha256: "hash123".to_string(),
            size_bytes: 512,
            mime_type: "application/json".to_string(),
        },
    );

    let params = ProtocolSubmitParams {
        session_id: "session-123".to_string(),
        step_id: "step-1".to_string(),
        checklist_evidence: evidence,
        artifacts,
    };

    let json = serde_json::to_string(&params).unwrap();
    let deserialized: ProtocolSubmitParams = serde_json::from_str(&json).unwrap();
    assert_eq!(params.session_id, deserialized.session_id);
    assert_eq!(params.step_id, deserialized.step_id);
    assert_eq!(params.checklist_evidence, deserialized.checklist_evidence);
    assert_eq!(params.artifacts.len(), deserialized.artifacts.len());
}

#[test]
fn error_display_impl() {
    let manifest_err = ManifestError::SchemaNotFound(PathBuf::from("/missing/schema.json"));
    assert!(format!("{}", manifest_err).contains("Schema file not found"));

    let validation_err = ValidationError::DuplicateStepId("step-1".to_string());
    assert!(format!("{}", validation_err).contains("Duplicate step_id"));

    let ledger_err = LedgerError::Corrupt {
        line: 42,
        message: "invalid json".to_string(),
    };
    assert!(format!("{}", ledger_err).contains("line 42"));
    assert!(format!("{}", ledger_err).contains("invalid json"));

    let artifact_err = ArtifactError::HashMismatch {
        expected: "abc".to_string(),
        actual: "def".to_string(),
    };
    assert!(format!("{}", artifact_err).contains("Hash mismatch"));

    let fsm_err = FsmError::SessionNotFound("session-123".to_string());
    assert!(format!("{}", fsm_err).contains("Session not found"));

    let contract_err = ContractError::ValidationFailed("field missing".to_string());
    assert!(format!("{}", contract_err).contains("Validation failed"));

    let gateway_err = GatewayError::InvalidRequest("bad param".to_string());
    assert!(format!("{}", gateway_err).contains("Invalid request"));
}

#[test]
fn error_trait_bounds() {
    fn assert_error_bounds<E: std::error::Error + Send + Sync + 'static>() {}

    assert_error_bounds::<ManifestError>();
    assert_error_bounds::<ValidationError>();
    assert_error_bounds::<LedgerError>();
    assert_error_bounds::<ArtifactError>();
    assert_error_bounds::<FsmError>();
    assert_error_bounds::<ContractError>();
    assert_error_bounds::<GatewayError>();
}

#[test]
fn fsm_event_with_all_event_types_roundtrip() {
    let event_types = vec![
        FsmEventType::SessionStarted {
            pipeline_id: "pipe-1".to_string(),
            manifest_version: "1.0".to_string(),
            profile_sha256: None,
        },
        FsmEventType::MilestoneSubmitted {
            evidence_keys: vec!["a".to_string()],
            iteration: 1,
        },
        FsmEventType::MilestoneAccepted {
            next_step_id: Some("step-2".to_string()),
        },
        FsmEventType::MilestoneRejected {
            rejected_items: vec!["a".to_string()],
            reason: "incomplete".to_string(),
            iteration: 1,
        },
        FsmEventType::StepAdvanced {
            from_step: "step-1".to_string(),
            to_step: "step-2".to_string(),
        },
        FsmEventType::CircuitBreakerTriggered {
            breaker: CircuitBreakerType::RepetitiveLoop,
            details: serde_json::json!({"count": 3}),
        },
        FsmEventType::SessionCompleted {
            final_artifact_path: "/out.json".to_string(),
            output_sha256: None,
        },
        FsmEventType::SessionFailed {
            reason: "error".to_string(),
            final_step: "step-1".to_string(),
        },
    ];

    for event_type in event_types {
        let event = FsmEvent {
            event_id: sample_uuid(),
            timestamp: sample_datetime(),
            session_id: "sess-1".to_string(),
            event_type: event_type.clone(),
            step_id: Some("step-1".to_string()),
            payload: serde_json::json!({}),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: FsmEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(
            format!("{:?}", event.event_type),
            format!("{:?}", deserialized.event_type)
        );
    }
}

// --- WP-2 transcript notary: additive-field wire compatibility -------------

/// NOT-13: a ledger written BEFORE this feature (no `profile_sha256` /
/// `output_sha256` keys) still deserializes, with the new fields defaulting to
/// `None`.
#[test]
fn pre_feature_events_deserialize_with_none() {
    let started: FsmEventType = serde_json::from_str(
        r#"{"type":"SessionStarted","pipeline_id":"p","manifest_version":"1"}"#,
    )
    .unwrap();
    match started {
        FsmEventType::SessionStarted { profile_sha256, .. } => assert_eq!(profile_sha256, None),
        other => panic!("expected SessionStarted, got {other:?}"),
    }

    let completed: FsmEventType =
        serde_json::from_str(r#"{"type":"SessionCompleted","final_artifact_path":"/out.json"}"#)
            .unwrap();
    match completed {
        FsmEventType::SessionCompleted { output_sha256, .. } => assert_eq!(output_sha256, None),
        other => panic!("expected SessionCompleted, got {other:?}"),
    }
}

/// NOT-12: a hash-less event serializes byte-identically to the pre-feature
/// shape — `skip_serializing_if` drops the `None` field, so an existing ledger's
/// bytes are unchanged and the transcript root over an old session is stable.
#[test]
fn hashless_events_serialize_without_the_new_key() {
    let started = serde_json::to_string(&FsmEventType::SessionStarted {
        pipeline_id: "p".to_string(),
        manifest_version: "1".to_string(),
        profile_sha256: None,
    })
    .unwrap();
    assert_eq!(
        started,
        r#"{"type":"SessionStarted","pipeline_id":"p","manifest_version":"1"}"#
    );

    let completed = serde_json::to_string(&FsmEventType::SessionCompleted {
        final_artifact_path: "/out.json".to_string(),
        output_sha256: None,
    })
    .unwrap();
    assert_eq!(
        completed,
        r#"{"type":"SessionCompleted","final_artifact_path":"/out.json"}"#
    );
}

#[test]
fn protocol_submit_result_advanced_roundtrip() {
    let result = ProtocolSubmitResult::Advanced {
        next_step: Some(StepInfo {
            step_id: "step-2".to_string(),
            name: "Step Two".to_string(),
            exit_checklist: vec!["check1".to_string()],
        }),
        session_state_json: serde_json::json!({}),
    };

    let json = serde_json::to_string(&result).unwrap();
    let deserialized: ProtocolSubmitResult = serde_json::from_str(&json).unwrap();

    match (result, deserialized) {
        (
            ProtocolSubmitResult::Advanced { next_step: a, .. },
            ProtocolSubmitResult::Advanced { next_step: b, .. },
        ) => {
            assert_eq!(a.unwrap().step_id, b.unwrap().step_id);
        }
        _ => panic!("variant mismatch"),
    }
}

#[test]
fn protocol_submit_result_rejected_roundtrip() {
    let result = ProtocolSubmitResult::Rejected {
        rejected_items: vec!["item1".to_string()],
        reason: "incomplete".to_string(),
        current_step: StepInfo {
            step_id: "step-1".to_string(),
            name: "Step One".to_string(),
            exit_checklist: vec!["item1".to_string()],
        },
    };

    let json = serde_json::to_string(&result).unwrap();
    let deserialized: ProtocolSubmitResult = serde_json::from_str(&json).unwrap();

    match (result, deserialized) {
        (
            ProtocolSubmitResult::Rejected {
                rejected_items: a,
                reason: ra,
                ..
            },
            ProtocolSubmitResult::Rejected {
                rejected_items: b,
                reason: rb,
                ..
            },
        ) => {
            assert_eq!(a, b);
            assert_eq!(ra, rb);
        }
        _ => panic!("variant mismatch"),
    }
}
