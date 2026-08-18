// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Contract validator using jsonschema

use protocol_types::ContractError;

pub fn validate_output(
    payload: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), ContractError> {
    let compiled = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .build(schema)
        .map_err(|e| ContractError::SchemaError(e.to_string()))?;

    let errors: Vec<String> = compiled
        .iter_errors(payload)
        .map(|e| format!("{}: {}", e.instance_path(), e))
        .collect();
    if !errors.is_empty() {
        return Err(ContractError::ValidationFailed(errors.join("; ")));
    }
    Ok(())
}
