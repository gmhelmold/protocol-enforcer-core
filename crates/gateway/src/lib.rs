// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Protocol gateway crate - MCP Gateway (stdio transport)

pub mod error_map;
pub(crate) mod introspect;
pub(crate) mod paths;
mod recovery;
pub mod server;
pub mod transport;
pub(crate) mod wire;

pub use server::ProtocolServer;
pub use transport::run_gateway_server;
