# Changelog

All notable changes to this project are documented here. Format loosely
follows [Keep a Changelog](https://keepachangelog.com/).

## 0.1.1

- Fix: `protocol-artifacts`' path validation (`is_safe_relative_path`) now
  correctly accepts a composite `step_id` in `macro_id/sub_state_id` form —
  the shape the enforcer actually passes for the current step — while still
  rejecting `..` segments, empty segments, backslashes, and absolute paths in
  any segment. A prior stricter single-segment check would have rejected a
  legitimate composite `step_id`.

## 0.1.0

- Initial public release of the Apache-2.0 open core: `protocol-types`,
  `protocol-library`, `protocol-manifest`, `protocol-ledger`,
  `protocol-artifacts`, `protocol-fsm`, `protocol-contract`,
  `protocol-gateway`, `protocol-notary`.
- MCP stdio gateway (`protocol-gateway`) exposing `protocol_start`,
  `protocol_submit_milestone`, `protocol_get_state`, `protocol_store_artifact`,
  `protocol_profile_list`, `protocol_profile_show`.
- RFC 6962 transcript root, Ed25519 witness signing (`protocol-notary`), and
  the `human_approval` gate.
