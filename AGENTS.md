# Agents

Read these before touching anything, in this order:

1. **`ARCHITECTURE.md`** — the design of record. How the pipeline works, what each file holds,
   and the decisions a plausible-looking change would undo by accident. If your change
   contradicts it, update it in the same pull request.
2. **`CONTRIBUTING.md`** — the rules. Branch and pull-request workflow, conventional commits,
   code conventions, the determinism contract, and how engine fine-tuning is done.
3. **`HANDOFF.md`** — live state between sessions: open questions, decisions taken in
   conversation, and the recurring bug class to watch for. Update it as you go; it is the only
   document that is allowed to be about the present moment rather than about the code.

Three things that are not negotiable and are the ones most often got wrong:

- **Never delete or weaken a failing test.** Leave it `#[ignore]`d with a
  `TODO: fine tune to catch this` reason. `CONTRIBUTING.md` explains why.
- **Document through naming, not comments.** Rustdoc is timeless: no changelogs, no phase
  numbers, no review-finding identifiers, no record of what the code used to do.
- **Determinism is a contract.** A pinned seed reproduces byte-identical output under any
  `RAYON_NUM_THREADS`. Verify with the full suite under both thread counts, not one.
