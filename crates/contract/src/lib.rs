// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Protocol contract crate - output contract enforcement

pub mod validator;

pub use validator::validate_output;
