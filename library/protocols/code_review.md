## Protocol: Self Code Review

Before declaring a change done, review your own diff against:
- Correctness: does it do exactly what the checklist item requires?
- Tests: is the new behavior covered by a test that would fail without the change?
- Blast radius: what else touches this code path?
- Simplicity: can it be smaller? any dead code introduced?
- Errors: are failure modes handled, not swallowed?

Produce a short written verdict per item before advancing.
