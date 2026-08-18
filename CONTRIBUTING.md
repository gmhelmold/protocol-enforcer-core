# Contributing

## License

This project is Apache-2.0 (see `LICENSE`). By submitting a contribution, you
agree it is licensed under Apache-2.0 and that you have the right to submit it
(inbound = outbound: your contribution is licensed to the project on the same
terms the project licenses code to you).

## Before opening a PR

The CI gate (`.github/workflows/ci.yml`) runs on every push and PR against
`main` and must pass:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test  --workspace
```

Run all three locally before opening a PR. `clippy` runs with
`-D warnings` — a warning fails the build, same as an error.

## Workspace conventions

- Every tracked `.rs` file carries an `SPDX-License-Identifier: Apache-2.0` +
  `Copyright 2026 Gustavo Schneiter` header as its first two lines (a blank
  line before any `//!` module doc that follows).
- No crate here depends on anything outside this workspace's own crates
  (see the crate table in the README) other than published crates.io
  dependencies.
- Keep comments self-contained: don't cite documents, work-package IDs, or
  rule numbers from specs that aren't in this repository — a reader here
  can't follow that link.

## Commit / PR style

- One logical change per PR where practical; keep the diff reviewable.
- Write commit messages that explain *why*, not just *what*.
- No secrets, tokens, or personal absolute paths in committed files. The demo
  Ed25519 seed in `profiles/human-gate-demo.yaml`
  (`0707070707070707070707070707070707070707070707070707070707070707`) is a
  deliberately published, worthless demo key — that one is fine to keep.
