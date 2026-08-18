// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! MCP Gateway server entry point.
//! Serves `ProtocolServer` (see `server.rs`) over stdio to completion.

use rmcp::ServiceExt;

use crate::server::ProtocolServer;

/// Construct the MCP server and serve it over stdio until the connected
/// client disconnects (or the process is terminated). Blocks on the
/// transport loop.
pub async fn run_gateway_server() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Protocol Enforcer MCP Gateway starting (stdio)");
    tracing::info!("MCP tools: protocol_start, protocol_submit_milestone, protocol_get_state");

    let server = ProtocolServer::new();
    let service = server.serve(rmcp::transport::stdio()).await?;

    tracing::info!("Protocol Enforcer MCP Gateway ready, serving over stdio");
    service.waiting().await?;

    tracing::info!("Protocol Enforcer MCP Gateway shutting down");
    Ok(())
}
