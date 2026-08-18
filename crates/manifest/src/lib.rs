// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Protocol Manifest - nested profile loader + strict validator.
//!
//! `load_profile` + `validate_profile` are the sole loader/validator on the
//! served path.

pub mod editor;
pub mod hook_validator;
pub mod profile_loader;
pub mod profile_manager;
pub mod profile_validator;

pub use editor::{EditError, ProfileEditor};
pub use hook_validator::validate_hooks;
pub use profile_loader::load_profile;
pub use profile_manager::{is_safe_name, ProfileManager};
pub use profile_validator::validate_profile;
