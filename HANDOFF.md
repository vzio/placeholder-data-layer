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
| P4.5 UUID-global identity + preserve opt-in | done | `5bbab5d` |
| P5 compliance checks, retry loop, fidelity report | done | `1d08cc6` |
| P6 TS client, GHCR CI, README, TODO sweep | done | `b323f27` |

Repo: private `vzio/placeholder-data-layer`, pushed to `main` through `aff1ec9`.
225 tests green under both `RAYON_NUM_THREADS=1` and `=8`, plus 1 ignored (see below).
by P6, which touched derive attributes and documentation only, nothing behavioural.
~8,100 lines under `crates/*/src` including in-file test modules, ~4,200 of integration tests.

### Uncommitted right now
Nothing. The tree is clean at `b323f27` and pushed. **All six phases in `PLAN.md`'s
implementation order are done.** What remains is `PLAN.md` §12's deferred backlog, which is the
engine-tuning work, not infrastructure.

### Open questions for the owner
Answered with a default rather than blocked on, since they were raised while you were asleep.
Each is a one-line change if the default is wrong.

1. **GHCR image name** is `${{ github.repository }}`, so `ghcr.io/vzio/placeholder-data-layer`.
   If the platform compose file expects something else, that is the line to change.
2. **The client exposes `profile()`, `detectors()` and `jobs()`** beyond the `anonymize()` that
   `PLAN.md` §11 shows. `detectors()` and `jobs()` throw where the other two return a result
   union, on the grounds that they are introspection rather than pipeline calls. The asymmetry is
   defensible but it is a public surface, so worth a look.
3. **`PLAN.md` §6's dependency list does not mention `ts-rs`.** Implementation work does not edit
   `PLAN.md`, so it was left alone; a doc-sync pass may want it.
4. **Large integers cross the client as digit strings.** `Value`'s integer arm is
   `number | string` in TypeScript because `JSON.parse` cannot represent an i64 beyond 2^53 and
   key minting preserves input magnitude. Backend Core must expect either form. The alternative
   was clamping the mint domain below 2^53, which is an engine change and was not taken.
5. **`docker build` is unverified** — no docker binary in the sandbox. The Dockerfile is
   unchanged from before P6 and its build-stage command runs clean, but the image itself is
   proven only when CI first runs.

## The working loop

Plan → implement (Sonnet) → review (Opus, adversarial) → fix (Sonnet) → verify → commit → next.
Reads and extraction go to Haiku. A refactor checkpoint is considered *before* each new phase.
Tokens are the binding constraint, not time; the 5h window is not readable programmatically, so
ask the user to read it off their statusline at phase boundaries.

Every phase so far has come back from review with real defects, several of them critical.
Budget review-and-fix at roughly half of each phase's cost. Do not skip the review pass.

## The recurring bug class — and why P5 does not close it

**"Redraw the characters I recognize, keep the rest verbatim."** It has produced a PII leak in
three separate phases:

- P4: a text key `"CONTRATO-SÃO-PAULO-001"` minted to `"PQBRPBRI-FÃV-WDNVI-863"` — city intact.
- P4: values not matching their column's expected shape returned byte-identical, so
  `"NAO TEM TELEFONE"` and `"NASCIDO EM 1972"` passed through a phone and a date column.
- P4.5: one UUID among 63 free-text values routed the whole column to `MintUuid`, and non-hex
  characters passed through — `joao.silva@example.com` → `jodo.silva@5x5mpl4.com`.

Each looked locally reasonable, because preserving structure is usually right for
format-preserving transformation. Each time, the unrecognised characters *were* the PII.

**P5's leakage check does not detect this class.** The comparison is whole-value: an output cell
is flagged only when it equals a source cell outright. Partial verbatim survival passes. At seed 1
the current fixture produces `"NASCIDO EM 1972"` → `"NASCIDOx EM 1309"`, ten of fifteen characters
surviving in place, and the check reports `pass`.

This is deliberate, not an oversight. The obvious fix — flagging a long verbatim run shared
between input and output — collides with the engine's design, because `PreservePrefix` keeps the
phone prefix verbatim *on purpose*. Calibrating that threshold is engine fine-tuning. The gap is
stated in `leakage`'s own rustdoc, recorded in `PLAN.md` §12, and pinned by
`a_value_that_survives_in_large_part_is_flagged_as_leaked`, which is `#[ignore]`d and asserts the
behaviour we want. Running it with `--ignored` fails today; making it pass is the fine-tuning
goal, and un-ignoring it is the proof.

Note also that `tests/leakage.rs` (P4) does substring containment across 40 seeds, which is a
*stronger* check than the enforced runtime one — just test-only.

## P6 — what it delivered

Last phase. `PLAN.md` §11 is the client design.

- **TypeScript client** — types generated rather than hand-written, `DataLayer.connect()` as a
  static async factory probing `/health` so a misconfigured base URL fails at construction rather
  than at first anonymize.
- **GHCR image build-and-push in CI.** Each layer of the platform ships its own image and tag; a
  thin `placeholder-platform` repo composes them, with no submodules. This repo owns only the
  build-and-push half.
- **README** — what the service is, the two endpoints, the four spec requirements and how the
  tests prove them, and an honest statement of what the PoC engine does not do.
- **TODO sweep** — `grep "TODO: fine tune"` enumerates the engine backlog for the team that picks
  this up; make sure every entry names its gap in one line and reads cold.
- **Final review pass** over the whole repo.

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

- **Failing tests stay in the suite, ignored, never deleted or weakened.** A test encoding the
  contract we want beats a passing test encoding a weakened one. Tag the gap
  `#[ignore = "TODO: fine tune to catch this — <gap>"]` so one grep enumerates the backlog. This
  is opportunistic: bank a gap found while doing the phase's own work, never go prospecting.
- **Infrastructure is finished; the engine is not.** Enums, public APIs, endpoints, job/worker
  threading, retry wiring and report types are long-lived surfaces a smarter engine plugs into
  without reshaping them — prefer the durable signature over the clever one. Engine accuracy is
  explicitly a later team's work.
- **The top-level verdict covers enforced checks only.** Linkage warns on its own entry. With the
  threshold at 5 and `k_observed` a global minimum, letting a `Warn` propagate would make almost
  every real dump non-`pass` and leave a consumer gating on `verdict == "pass"` rejecting
  everything while a genuine leak looked identical.

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
