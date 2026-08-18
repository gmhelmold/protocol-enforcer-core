//! A profile can wire a checklist criterion to a verify hook — a driver-side
//! shell command that checks a fact and produces evidence for that criterion.
//! Hooks like that are never MCP tools: the model can only see and call the
//! fixed tool set the gateway registers (`protocol_start`,
//! `protocol_submit_milestone`, …), never an arbitrary hook by name. This test
//! pins that guarantee: it asserts the model-facing tool manifest carries the
//! fixed tools and carries nothing that looks like a verify/attest hook or
//! oracle under any spelling, so a hook can never become a callable tool.

use protocol_gateway::server::ProtocolServer;

#[test]
fn ac38_verify_fact_is_absent_from_the_model_facing_tool_manifest() {
    let tools = ProtocolServer::new().model_facing_tool_names();

    // NON-VACUITY: the manifest is the REAL surface — it carries the fixed MCP
    // tools the model drives (so an empty/broken list can't vacuously pass).
    for expected in [
        "protocol_start",
        "protocol_submit_milestone",
        "protocol_get_state",
        "protocol_store_artifact",
        "protocol_profile_list",
        "protocol_profile_show",
    ] {
        assert!(
            tools.iter().any(|t| t == expected),
            "the model-facing manifest must expose the fixed tool '{expected}': {tools:?}"
        );
    }

    // NONE of the tools is the verify-fact oracle, the verify hook, or the
    // attest hook — under any spelling. The model cannot list or invoke them.
    for forbidden in [
        "verify-fact",
        "verify_fact",
        "verifyfact",
        "genesis-verify",
        "genesis_verify",
        "genesis-attest",
        "genesis_attest",
        "probe",
        "oracle",
    ] {
        assert!(
            !tools.iter().any(|t| t.to_lowercase().contains(forbidden)),
            "'{forbidden}' must NOT be a model-facing tool: {tools:?}"
        );
    }
}
