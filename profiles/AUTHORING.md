# Editing profiles (LLM-first)

A **profile** is one YAML file in `profiles/<name>.yaml` that defines the state
machine the enforcer drives: macro-states → sub-states → the exit gate. To change
states, sub-states, prompts, or gate criteria, edit the YAML and validate. This
file is the recipe an LLM follows when a human says "change state X" / "add a
step" / "tweak that prompt".

## Always validate after editing

```bash
target/release/protocol-enforcer validate --manifest profiles/<name>.yaml
# or from source: cargo run -q --release -p protocol-cli -- validate --manifest profiles/<name>.yaml
```

Valid → `✅ Profile is valid` + a state/sub-state summary. Invalid → the error
names the exact path, e.g. `pipeline[0].sub_states[1].type: unknown variant
'bogus'` — go fix that path and re-run. Never ship a profile that doesn't validate.

To see what the running gateway actually serves for a profile, use the MCP tool
`protocol_profile_show(name)` (read-only).

## Shape (the whole schema)

```yaml
name: "quick-bug-fix"        # MUST match the filename stem
version: "1.0.0"
description: "one line"
protected: false             # true = engine refuses to modify/delete it
settings:
  oscillation_detection_threshold: 3   # loop-guard
  auto_loop_on_checklist_failure: true # failed gate loops back
pipeline:                    # ORDER = execution order (top → bottom)
  - state_id: understand     # a macro-state
    name: "Understand Bug"
    description: "..."
    system_prompt: |         # OPTIONAL big prompt injected for this macro
      Multi-line markdown goes here, indented under the `|`.
    enabled: true
    max_iterations: 3
    icon: "🐛"
    sub_states:              # ORDER = execution order within the macro
      - id: reproduce
        type: execute        # inject | execute | review | checklist
        name: "Reproduce Bug"
        description: "..."
        enabled: true
      - id: checklist
        type: checklist
        name: "Checklist Gate"
        enabled: true
        criteria:            # ONLY on checklist; each item is a REQUIRED key
          - bug_reproduced
          - root_cause_identified
```

## Sub-state types

| type | what it does |
|------|--------------|
| `inject`    | injects context/prompt into the model; no work performed |
| `execute`   | the agent uses tools freely to do the work of this step |
| `review`    | the agent reviews its own work |
| `checklist` | **gate**: the engine only advances when every `criteria` key is PRESENT in the submitted evidence. It checks presence, not truth (the passivity invariant) — a criterion is a required checkbox, not a verified fact |
| `human_approval` | **gate a model cannot open**: the engine issues a random challenge on entry and only advances when a HUMAN signs it with the key named in `approver_pubkey`. The one type whose evidence is verified, not merely observed |

## `human_approval` — the gate only a person can open

Use it where a machine check cannot substitute for a decision: shipping, deleting,
paying, anything an operator must own. Two extra fields, both only valid here:

```yaml
      - id: human_gate
        type: human_approval
        name: "Human Sign-Off"
        enabled: true
        approver_pubkey: "ea4a…d22c"   # REQUIRED: hex Ed25519 PUBLIC key. Never a seed.
        approval_prompt: |             # optional: what the human is shown
          Review the change summary, then sign the challenge.
```

- `approver_pubkey` is **required** here and **forbidden** on every other type —
  `validate` fails both ways. Mint a key with `notary-keygen`; put the **public** key
  in the profile and keep the seed on the approver's machine.
- No `criteria` (that is checklist-only). The macro still needs its trailing
  `type: checklist` sub-state after the gate.
- Flow: the agent relays the `approval_challenge` from its step payload → the human
  runs `protocol-enforcer approve --session … --macro … --sub-state … --challenge …
  --key …` → the agent submits the printed hex as evidence `approval_signature`.
- Every re-entry (loop-back, retry) issues a NEW challenge, so old signatures die.
- **If the agent can read the key file, this gate is presence-only.** Custody is the
  whole security property — see `docs/SPEC_human_approval.md`.

Working example: `profiles/human-gate-demo.yaml` (ships with a published demo key).

## The 3 things that break edits

1. `system_prompt: |` — the text must stay indented under the `|`; it is literal
   multi-line content.
2. `criteria:` belongs **only** under a `type: checklist` sub-state;
   `approver_pubkey`/`approval_prompt` belong **only** under `type: human_approval`.
3. **Order matters** — states run top-to-bottom and so do sub-states; reordering
   the blocks reflows the pipeline.

## Common edits

- **Add a state**: copy a whole `- state_id:` block, change its ids/names, place
  it where it should run.
- **Add a sub-state**: add an item under that state's `sub_states:`.
- **Change a prompt**: edit that state's `system_prompt:` block.
- **Change a gate**: edit the checklist sub-state's `criteria:` list.
- **New profile**: copy `default.yaml` → `profiles/<new>.yaml`, set `name:` to
  match the filename, edit, validate. (`default.yaml` is `protected` — never edit
  it in place; use it as a template.)

## External-probe profiles (ship fail-closed)

Some profiles wire a verify hook that shells an **external probe** — a program
outside this repo that speaks the adapter's JSON-stdin/stdout-verdict contract.
The `genesis-dependency*` family is the current example: their verify hooks
(`library/hooks/scripts/genesis-verify-dependency*.mjs`) call an Atlas
symbol-reverse oracle (`verify-fact.mjs` + a SCIP index) that is **not shipped
here**.

Rules for this category:

- **They do NOT work out-of-the-box.** The external probe must be installed and
  wired before any fact can be proven.
- **They ship fail-closed.** The probe/scipPath args are left as **empty strings**
  (`probe: ""`, `scipPath: ""`), never a hardcoded personal absolute path. The
  adapter's guard (`if (!probe || !scipPath) → strip`) then fires cleanly and the
  hook admits **zero** facts — a spawn on a dead path would instead error. An
  empty string is the correct inert placeholder.
- **Every such profile carries a top-of-file `EXTERNAL-PROBE PROFILE` comment**
  stating it needs an external probe and that as shipped it admits nothing, so no
  one mistakes it for functional.
- To enable one: point `probe`/`scipPath` at your installed probe + index (via
  env or a local edit you do not commit), then `validate`.

## Recipe: "human asks, model does"

1. Read the target `profiles/<name>.yaml`.
2. Make the requested edit (state / sub-state / prompt / criteria).
3. Run `validate`; if it fails, fix at the reported path and re-run.
4. The change is live for any new session the gateway starts from that profile.
