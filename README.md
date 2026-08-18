# Protocol Enforcer — core

A **passive, deterministic MCP server** that serves a **nested, force-ordered
state machine** to external AI agents. The enforcer holds no LLM: it reveals the
next step only after the current one is submitted, so an agent's attention is
spent on one bounded step at a time instead of a whole protocol at once. Rust
workspace, Cargo, MCP over stdio (`rmcp`).

The design principle: an LLM is not disobedient, it is *context-limited*. Break a
protocol into states, reveal each only when the previous is finished, and the
model follows the protocol because at every moment it only has to.

This repository is the **Apache-2.0 open core** — the enforcement engine and its
served path. It builds and runs standalone. (The reference LLM orchestrator and
the operator CLI are maintained separately and are not part of this repository.)

## Crates

| Crate | Role |
|---|---|
| `protocol-types` | Canonical types: the `profile` model (`Profile`/`StateDef`/`SubStateDef`/`SubStateType`/`Injection`/`VerifyCheck`), `Position`, `SessionState`, errors. |
| `protocol-library` | Resolves `inject.skill` / `inject.protocol` names to text under `library/{skills,protocols}/<name>.md`. |
| `protocol-manifest` | `load_profile` + strict `validate_profile`; `ProfileManager` (profile CRUD). |
| `protocol-ledger` | Append-only durable ledger + RFC 6962 `merkle` transcript root (computed once per session at completion, not per step). |
| `protocol-notary` | Ed25519 transcript-root witness seal (`sign_root`/`verify_root`) and the `human_approval` gate primitives (challenge + `sign_approval`/`verify_approval_hex`). |
| `protocol-artifacts` | Artifact offloading through the enforcer's own content-addressed store (streaming SHA-256). |
| `protocol-contract` | JSON-Schema output-contract validation. |
| `protocol-fsm` | `ProfileFsmEngine` — the stepping engine (forced macro/sub-state order, presence-only checks, circuit breakers, output-contract) + session recovery. |
| `protocol-gateway` | The MCP tools over stdio driving the engine; the `protocol-gateway` binary. |

No crate here depends on anything outside this workspace's own crates.

## MCP tools

The gateway exposes six tools over stdio:

- `protocol_start` — start a session from a profile; returns the first injected step.
- `protocol_submit_milestone` — submit a sub-state (with checklist evidence / final `output`); returns the next injected step, a rejection, or completion.
- `protocol_get_state` — read a session's current position/status.
- `protocol_store_artifact` — offload an artifact **through the enforcer's own store**: the enforcer writes the bytes, computes an **enforcer-authored** `sha256`, tracks the ref (ledgered, survives recovery), and re-verifies every tracked artifact's integrity before the session may complete.
- `protocol_profile_list` — list available profiles.
- `protocol_profile_show` — show a single profile's definition.

## Build & test

```bash
cargo build --workspace
cargo test  --workspace
```

Run the gateway as an MCP stdio server:

```bash
cargo run -p protocol-gateway --bin protocol-gateway
```

## Transcript notary

Every completed session yields one 64-hex **transcript root** — an RFC 6962
Merkle tree over the ledger's lines — returned in the completion response. It
commits to the profile in force, every step, every offloaded artifact, and the
final `output_contract` deliverable, yet reveals none of them, and is verifiable
by a third party. The `protocol-notary` and `protocol-ledger::merkle` crates
expose the primitives to recompute the root, build/verify inclusion proofs, and
witness-sign the root with Ed25519.

## Performance

Measured, not asserted. Hardware: one 2019 Intel Core i7-9750H (6c/12t), APFS
SSD, `--release`. Order-of-magnitude on a laptop CPU; the ratios carry the
argument.

| What | Number | Notes |
|---|---|---|
| Engine work per step (`submit_milestone`) | **~11.2 µs** CPU | In-memory ledger; no LLM, no external command on the path. |
| Per-step wall cost, durable ledger | **~16–20 ms** | The entire delta is one `fdatasync` — an O(1) single-line append for crash-durability, *not* compute. |
| Throughput, in-memory | **~1,812 sessions/s · ~56k steps/s** (median of 3) | One core, 31-step sessions, includes per-session profile validate. |
| Throughput, durable fsync path | **~36–43 steps/s** | Three orders down from in-memory — entirely the per-step fsync. |
| RFC-6962 transcript root | **O(n)**, ~1.8 µs/leaf; ~18 ms at 10,000 leaves | Computed once at completion, never on the per-step path. |
| Gateway idle RSS | **~2.67 MiB** (shipped binary ~2.89 MiB) | Empty session registry. |
| Per retained session | **~27 KiB** | Cost of keeping a live session in the registry. |

Reproduce:

```bash
cargo bench -p protocol-ledger --bench hotpath
cargo bench -p protocol-fsm    --bench hotpath
cargo run --release -p protocol-fsm     --example throughput  -- 1000
cargo run --release -p protocol-gateway --example gateway_mem -- 500
```

The honest headline: the enforcer adds **no LLM and no command execution** to the
hot path and only **~11 µs of CPU** per step; the measurable per-step cost is one
deliberate durable fsync (~16–20 ms here) — still ~1000× cheaper than a single
model round-trip, but not literally zero, and filesystem-dependent.

## License

Apache-2.0. See [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE). Every source file
carries an `SPDX-License-Identifier: Apache-2.0` header.
