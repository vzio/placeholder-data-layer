# Handoff — working state

Live state of the build. `PLAN.md` is the design of record and wins on any disagreement;
`AGENTS.md` holds the conventions. This file holds what is true *right now* and what is open.

## Where the build is

| Phase | State | Commit |
|---|---|---|
| P0 skeleton | done | `13b6b95` |
| P1 validation, equivalence classes, jobs/workers, passthrough | done | `2b5c152` |
| P2 classification + profiling | done | `52e5789` |
| P3 anomaly detection + corruption synthesis | done | `5316fdc` |
| P4 ledger minting, value-local transforms, re-injection | done | `74de0d8` |
| P4.5 UUID-global identity + preserve opt-in | **implemented, reviewed, fixed, uncommitted** | — |
| P5 compliance checks, retry loop, fidelity report | not started | — |
| P6 TS client, GHCR CI, README, TODO sweep | not started | — |

Repo: private `vzio/placeholder-data-layer`, pushed to `main`.
204 tests green under both `RAYON_NUM_THREADS=1` and `=8`.
~4,300 lines of production Rust, ~4,500 of test code.

### Uncommitted right now
P4.5 plus its review fixes, plus a comment sweep removing review-finding identifiers
(`C1 regression:`, `M2 regression:`) and bug narration from source and test comments.
Verify `cargo fmt --all --check`, `clippy --workspace --all-targets -- -D warnings`, and 204
tests under both thread counts, then commit.

## The working loop

Plan → implement (Sonnet) → review (Opus, adversarial) → fix (Sonnet) → verify → commit → next.
Reads and extraction go to Haiku. A refactor checkpoint is considered *before* each new phase.
Tokens are the binding constraint, not time; the 5h window is not readable programmatically, so
ask the user to read it off their statusline at phase boundaries.

Every phase so far has come back from review with real defects, several of them critical.
Budget review-and-fix at roughly half of each phase's cost. Do not skip the review pass.

## The recurring bug class — check this first in P5

**"Redraw the characters I recognize, keep the rest verbatim."** It has produced a PII leak in
three separate phases:

- P4: a text key `"CONTRATO-SÃO-PAULO-001"` minted to `"PQBRPBRI-FÃV-WDNVI-863"` — city intact.
- P4: values not matching their column's expected shape returned byte-identical, so
  `"NAO TEM TELEFONE"` and `"NASCIDO EM 1972"` passed through a phone and a date column.
- P4.5: one UUID among 63 free-text values routed the whole column to `MintUuid`, and non-hex
  characters passed through — `joao.silva@example.com` → `jodo.silva@5x5mpl4.com`.

Each looked locally reasonable, because preserving structure is usually right for
format-preserving transformation. Each time, the unrecognised characters *were* the PII.

All three slipped past a green suite because **the leakage fixtures contained only well-formed
values**. P5's leakage check is the backstop for this class, so its fixtures must be dirty:
malformed documents, mixed-type columns, non-ASCII keys, framing text around valid values.

## P5 — what it has to do

Spec step 5 plus the report. `compliance.rs` still has `todo!()` in `enforced`,
`observe_linkage` and `score`.

**Enforced** (failure escalates `Strength` and retries):
- **Leakage** — no output value of a high-cardinality direct-identifier column appears in that
  column's source value set. Must *not* flag a `preserve`d column, which is intentional, but must
  still report it.
- **Referential** — the output's orphan-FK set is **identical to the input's**, not empty.
  Dangling FKs are the canonical production bug and requirement #3 says they survive.

**Reported, never enforced:**
- **Linkage** — observed minimum equivalence-class size on the quasi-identifier set, surfaced as
  `k_observed` with a `warn`. Forcing k would distort the distribution the engine exists to keep.
- **Distribution fidelity** — TVD per column. Expected 0 under value-local transforms.

Retries therefore only fire for leakage and referential. Escalation is
`Strength::next() -> Option<Self>`, so termination is proven by the type. If no attempt passes,
return the **best-scoring** attempt with a `fail` verdict and reasons — never silently the last.

Inputs P5 must consume that already exist: the preserve set and its reasons, per-column
classification and confidence, before/after distributions, per-kind anomaly rates, and the
equivalence classes.

## Decisions taken in conversation, with their reasons

Design decisions live in `PLAN.md`. These are the ones that shaped the work and are easy to
re-litigate without the reasoning:

- **HTTP, not FFI.** The serialisation cost FFI would save is real, but the process boundary is
  part of the platform's compliance argument, and a Rust abort would take the migration
  orchestrator down with it. Revisit only if the engine is ever embedded inline — an anonymising
  read proxy in front of the sandbox is the case that would justify it.
- **No Unix socket, no child-process spawning.** Both rejected on the docker-compose reality:
  every other service in the platform is a container with a port.
- **Everything mints, integers included.** `classify` short-circuits to a numeric type when every
  sampled value is numeric, so a CPF stored as `BIGINT` is invisible to it. Pass-through would be
  a bet the engine cannot win.
- **Row order is a contract.** Backend Core holds the dump it sent, so it recovers any mapping it
  needs by positional join. That is what makes "mint everything" cost nothing operationally, and
  it is why nothing may ever reorder rows.
- **Anomalies re-inject at their recorded source positions.** A row with both a bad email and a
  bad document is one bad data-entry event; redistributing them invents two broken rows. This also
  makes the count exactly `positions.len()` and removes competition between kinds for positions.
- **The sandbox is anonymous on its own; linkage requires production access.** With structure
  preserved, that is the honest claim to put in front of a DPO — not "this cannot be linked to
  anyone." The report should say which claim it is making.

## Open threads

- **Non-production row detection** (admin/test/seed rows). Designed in `PLAN.md` §12, not built.
  Score **per field, not per row**; a textbook-fake CPF beside a real-looking email is a real
  person whose document was never captured, and is exactly what pass-through would leak.
- **`Username` semantic type** (`PLAN.md` §12). Handles are direct identifiers but need
  handle-shaped replacements; mapping them onto `FullName` yields person names where a login
  belongs. A one-line change adding `username`/`user` to the `FullName` hints was reverted for
  this reason.
- **`MalformedName` lumps all-caps, all-lowercase and embedded-digits into one kind**, so the mix
  inside that kind is not preserved (source 86/14/0 → output 37/31/31). Splitting the kind would
  fix it; preserving source casing would not.
- **A thin `placeholder-platform` repo** (`PLAN.md` §12) — compose, cross-service integration
  tests, GHCR images, no submodules. This repo's half is the build-and-push step, scoped into P6.

## Verify before any commit

```
cargo build --workspace --locked
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
RAYON_NUM_THREADS=1 cargo test --workspace
RAYON_NUM_THREADS=8 cargo test --workspace
```

Both thread counts matter: the determinism guarantee is that a pinned seed reproduces
byte-identical output regardless of how rayon splits the work, and a single-thread run cannot
catch a ledger mutated under parallelism.
