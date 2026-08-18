// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Protocol Enforcer MCP Gateway binary entry point.
//!
//! Runs the Tier-1 passive MCP gateway over stdio. stdout is reserved for
//! the MCP JSON-RPC channel, so all logging goes to stderr.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    protocol_gateway::run_gateway_server().await
}
