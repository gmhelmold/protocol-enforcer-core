// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Nested profile loader.
//!
//! Loads a `protocol_types::Profile` (the nested pipeline/macro/sub-state
//! model) from a YAML file. This is the enforcer's sole served-path loader;
//! an earlier flat loader over a different manifest shape has since been
//! removed.

use protocol_types::{ManifestError, Profile};
use std::fs;
use std::path::Path;

/// Load a nested `Profile` from a YAML file at `path`.
///
/// # Errors
/// - `ManifestError::SchemaNotFound` if `path` does not exist / cannot be read.
/// - `ManifestError::Parse` if the file is not valid YAML or does not
///   deserialize into `Profile`.
pub fn load_profile(path: &Path) -> Result<Profile, ManifestError> {
    let content =
        fs::read_to_string(path).map_err(|_| ManifestError::SchemaNotFound(path.to_path_buf()))?;

    let profile: Profile = serde_yaml::from_str(&content)?;

    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tmp file");
        write!(f, "{content}").expect("write tmp file");
        f
    }

    const MINIMAL_PROFILE: &str = r#"
name: "minimal"
version: "1.0.0"
description: "minimal profile"
pipeline:
  - state_id: only
    name: "Only Macro"
    sub_states:
      - id: check
        type: checklist
        name: "Check"
        criteria:
          - "done"
"#;

    #[test]
    fn loads_valid_yaml_into_profile() {
        let f = write_tmp(MINIMAL_PROFILE);
        let profile = load_profile(f.path()).expect("should load");
        assert_eq!(profile.name, "minimal");
        assert_eq!(profile.pipeline.len(), 1);
    }

    #[test]
    fn missing_file_is_schema_not_found() {
        let err = load_profile(Path::new("/no/such/path/profile.yaml")).expect_err("must fail");
        assert!(matches!(err, ManifestError::SchemaNotFound(_)));
    }

    #[test]
    fn malformed_yaml_is_parse_error() {
        let f = write_tmp("not: [valid, yaml: because: of extra colons: ][");
        let err = load_profile(f.path()).expect_err("must fail");
        assert!(matches!(err, ManifestError::Parse(_)));
    }
}
