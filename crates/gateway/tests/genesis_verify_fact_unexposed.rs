//! SPIKE-1 WP-H1c · AC-38 — `verify-fact` (and the verify/attest hooks) is ABSENT
//! from the model-facing tool MANIFEST: the model cannot list or invoke it. This
//! is the OTHER half of passivity from AC-21 (`genesis_sound_manifest.rs`): AC-21
//! pins that a PROFILE's structural view hides the verify wiring; AC-38 pins that
//! the fixed MCP tool REGISTRY the model drives never contains the oracle as a
//! callable tool at all. Two distinct surfaces — a hidden-in-profile hook could
//! still, in principle, be a registered tool; this test forecloses that.
//!
//! The oracle is a hook (a driver-side shell command), never an MCP tool, so it is
//! structurally impossible for it to appear here — this test makes that guarantee
//! explicit and regression-proof.

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

    // AC-38: NONE of the tools is the verify-fact oracle, the verify hook, or the
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
            "AC-38: '{forbidden}' must NOT be a model-facing tool: {tools:?}"
        );
    }
}
