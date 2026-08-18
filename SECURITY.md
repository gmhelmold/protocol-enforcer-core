# Security policy

## Reporting a vulnerability

Please report security vulnerabilities through GitHub's private vulnerability
reporting for this repository, not through a public issue:

https://github.com/gmhelmold/protocol-enforcer-core/security/advisories/new

This opens a private advisory visible only to the maintainer until a fix is
ready. Include, where you have it:

- The crate(s) and version(s) affected.
- Steps to reproduce, or a minimal example.
- The impact you believe the issue has (e.g. what a malicious profile,
  malicious MCP client, or a compromised approver key could do).

## Scope

This repository is the Apache-2.0 open core: `protocol-types`,
`protocol-library`, `protocol-manifest`, `protocol-ledger`,
`protocol-artifacts`, `protocol-fsm`, `protocol-contract`, `protocol-gateway`,
`protocol-notary`. Issues in the separately-distributed reference orchestrator
or operator CLI are out of scope here.

## What's already a known, accepted trade-off

- The demo Ed25519 key shipped in `profiles/human-gate-demo.yaml`
  (`approver_pubkey: ea4a…d22c`) is a published, worthless demo key by design
  — its seed is `0707…07`. Do not report this as a leaked key.
- `human_approval` gate custody: if the agent driving a session can read the
  approver's private key file, the gate degrades to presence-only. This is
  documented behavior (see `profiles/AUTHORING.md`), not a vulnerability —
  keeping the key off any machine the agent can reach is the operator's
  responsibility.
