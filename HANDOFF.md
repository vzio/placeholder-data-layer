# Handoff — working state

Live state of the build. `ARCHITECTURE.md` is the design of record and wins on any disagreement;
`CONTRIBUTING.md` holds the rules. This file holds what is true *right now* and what is open —
it is the only document allowed to be about the present moment rather than about the code.

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

Repo: private `vzio/placeholder-data-layer`. 225 tests green under both `RAYON_NUM_THREADS=1`
and `=8`, plus 1 ignored (see below). ~8,100 lines under `crates/*/src` including in-file test
modules, ~4,200 of integration tests.

### Uncommitted right now
Nothing. The tree is clean and pushed. **All six phases of the build-out are
done**, and the repo has moved to the pull-request workflow in `CONTRIBUTING.md`. What remains
is engine tuning: `ARCHITECTURE.md`'s "What this build deliberately does not do", and
`grep "TODO: fine tune" crates/`.

### Open questions for the owner
Answered with a default rather than blocked on, since they were raised while you were asleep.
Each is a one-line change if the default is wrong.

1. **GHCR image name** is `${{ github.repository }}`, so `ghcr.io/vzio/placeholder-data-layer`.
   If the platform compose file expects something else, that is the line to change.
2. **The client exposes `profile()`, `detectors()` and `jobs()`** beyond `anonymize()`.
   `detectors()` and `jobs()` throw where the other two return a result union, on the grounds
   that they are introspection rather than pipeline calls. The asymmetry is defensible but it is
   a public surface, so worth a look.
3. **Large integers cross the client as digit strings.** `Value`'s integer arm is
   `number | string` in TypeScript because `JSON.parse` cannot represent an i64 beyond 2^53 and
   key minting preserves input magnitude. Backend Core must expect either form. The alternative
   was clamping the mint domain below 2^53, which is an engine change and was not taken.
4. **The image is verified.** Built and run with rootless podman: it serves `/health`, returns a
   full anonymize response for a seeded request, answers an unknown route with
   `application/problem+json`, and runs as uid 10001 rather than root. Podman needs
   two things Docker supplies implicitly — `unqualified-search-registries = ["docker.io"]` and a
   `policy.json` — neither of which is a defect in the `Dockerfile`.
   Open: the `Dockerfile` uses `RUN --mount=type=cache` with no `# syntax=docker/dockerfile:1`
   directive. BuildKit and buildah both accept it, so CI is unaffected, but a contributor on a
   daemon still using the classic builder gets a parse error rather than a slow build.

## The client's integer guarantee, and how it failed open

The TypeScript client must not round `i64` cell values. `Value::Int` is an `i64` and key minting
preserves the input's magnitude, so a bigint key column mints 19-digit pseudonyms whose spacing
is below the resolution of a double. Plain `JSON.parse` collapses distinct minted keys onto one
value, which would let the report assert an injectivity the caller never received.

The first implementation used `JSON.parse`'s reviver `source` argument. That argument exists only
on Node 21+. CI pins Node 20, where it is simply absent — so the guarantee **silently degraded to
plain lossy parsing** and the suite went red on `anonymize preserves three keys that only differ
past the 53rd bit`:

```
actual    5959995937546830000, 5959995937546830000, 5959995937546830000
expected '5959995937546829429', '5959995937546829430', '5959995937546829431'
```

CI did not break the fix; it caught that the fix only ever worked on the machine that wrote it.
A data-integrity guarantee that depends on a runtime feature, and whose absence is silent, is
worse than no guarantee: the caller cannot tell which behaviour it got.

It is now a source-text scan (`quoteOversizedIntegers` in `clients/typescript/src/index.ts`) that
quotes oversized integer literals before parsing, tracking string state so literals inside
strings are untouched. No runtime feature detection, so the behaviour is identical on every
supported Node.

**Watch for this shape.** `package.json` declares `"node": ">=18"`, so anything relying on a
newer runtime feature must either be polyfilled or the floor raised — never left to fail open.

### How the fix was verified
Node 20 could not be run locally: nixpkgs marks it insecure, so no binary is cached and a source
build fails nodejs' own `test-crypto-authenticated`. Verification is therefore static plus CI.
The whole runtime surface of the client is `AbortSignal.timeout` (Node 17.3+), `Number.isSafeInteger`,
`JSON.parse`/`JSON.stringify` with no reviver, and a sticky regex — nothing above the declared
floor, with `tsconfig` targeting ES2022. The suite passes on Node 24; CI's Node 20 job is the
binding proof.

### Also outstanding from this
- **CI pins `node-version: 20`, which is end-of-life.** nixpkgs refuses to build it without
  `NIXPKGS_ALLOW_INSECURE=1`. Bump to 22 or 24, and keep the declared `engines` floor honest.
- The `Dockerfile` uses `RUN --mount=type=cache` with no `# syntax=docker/dockerfile:1`
  directive. BuildKit and buildah accept it; the classic builder does not. One line, unasked.

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
stated in `leakage`'s own rustdoc, recorded in `ARCHITECTURE.md`, and pinned by
`a_value_that_survives_in_large_part_is_flagged_as_leaked`, which is `#[ignore]`d and asserts the
behaviour we want. Running it with `--ignored` fails today; making it pass is the fine-tuning
goal, and un-ignoring it is the proof.

Note also that `tests/leakage.rs` (P4) does substring containment across 40 seeds, which is a
*stronger* check than the enforced runtime one — just test-only.

## P6 — what it delivered

- **TypeScript client** — types generated rather than hand-written, `DataLayer.connect()` as a
  static async factory probing `/health` so a misconfigured base URL fails at construction rather
  than at first anonymize.
- **GHCR image build-and-push in CI.** Each layer of the platform ships its own image and tag; a
  thin `placeholder-platform` repo composes them, with no submodules. This repo owns only the
  build-and-push half.
- **README** — what the service is, the endpoints, and how to build and run it.
- **TODO sweep** — `grep "TODO: fine tune"` enumerates the engine backlog for the team that picks
  this up; make sure every entry names its gap in one line and reads cold.
- **Final review pass** over the whole repo.

## Decisions taken in conversation, with their reasons

Design decisions live in `ARCHITECTURE.md`. These are the ones that shaped the work and are
easy to re-litigate without the reasoning:

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

The engine backlog moved into `ARCHITECTURE.md` — "What this build deliberately does not do"
lists every gap, and "Designs worth keeping for when they get built" carries the reasoning for
the two whose obvious implementation is the wrong one. `grep "TODO: fine tune" crates/` finds
each gap at the line it applies to. Work them test-first, per `CONTRIBUTING.md`.

## Verify before any commit

See "Before opening a pull request" in `CONTRIBUTING.md`. In short: fmt, clippy with
`-D warnings`, and the full suite under both `RAYON_NUM_THREADS=1` and `=8` — a single-threaded
run cannot catch a ledger mutated under parallelism.
