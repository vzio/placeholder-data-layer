# Data Layer — Anonymization Engine: PoC Plan

Microservice #5 of the **Placeholder** database-migration platform (FASE 6 spec).
Owner: Lucas Vaz. Stack mandated by spec: **Rust + Axum**, HTTP transport.

## 0. Goal of this PoC

Get the *surrounding infrastructure* production-shaped: HTTP API, job and worker machinery,
in-memory storage, observability, and a functional test battery. The anonymization engine
itself ships as a deliberately modest v0 — correct against the four spec requirements, but
not yet smart. Quality there comes later, with time and pressure.

**Done means:** a private repo that builds, a documented public API, a first-pass engine, and
tests that demonstrate each spec requirement. Not: a polished product.

## 1. Scope

### In scope (spec, verbatim requirements)
The engine turns a production dump/sample into an anonymized but *statistically faithful*
dataset for the validation sandbox. It does not decide what to migrate and never touches
production: it receives source data and returns an anonymized version.

1. **Statistical distribution preserved per column.**
2. **Correlations between tables maintained.**
3. **Real anomalies reproduced** — not Faker-clean data; production bugs must survive into the sandbox.
4. **Referential integrity intact** — the same FK value maps to the same anonymized value in every table.

### Out of scope (spec)
Deciding schema, resolving type conflicts, writing to production. Those belong to the
orchestrator / AI Layer.

### Deferred past this PoC (TODOs left in code)
Database connectivity of any kind · persistence to disk · async job transport · Oracle Data
Safe · differential privacy · ML-assisted PII detection · AuthN/AuthZ (Backend Core owns RBAC).

## 2. Architectural decisions

| Decision | Choice | Rationale |
|---|---|---|
| Transport | Sync `POST /v1/anonymize`, whole dataset in, result out | Matches spec flow 1→6; all tables arriving together is what makes reqs #2 and #4 achievable |
| Async runtime | Tokio **as transport only** | Axum requires it; the engine crate has zero async |
| Jobs/workers | Owned OS worker threads pulling from a bounded `crossbeam-channel`; `JobRegistry` admits by capacity and the worker evicts on completion; the handler awaits a `tokio::sync::oneshot` | Real machinery now, not a stub. The worker thread is itself the blocking context, so no `spawn_blocking` and no nested runtimes. Eviction belongs to the worker, not the caller's future — otherwise a cancelled client frees the slot while the work is still queued |
| Parallelism | rayon over **columns**, inside a worker thread | Every engine pass is per-column; the pool keeps the HTTP reactor free |
| State | **None across requests.** `RunContext` owned by the request, dropped with the response | OpenResty `ngx.ctx` semantics; statelessness is the privacy posture, not just simplicity |
| Storage | In-memory only; in-flight jobs evicted on completion | No disk, nothing retained past the response |

### Determinism
`run_seed` is random per request unless pinned via `options.seed`. Pinning must reproduce
**byte-identical** output, including under a different rayon thread count — this is the
property that catches concurrency bugs, so it is a test, not an aspiration. It requires
`ChaCha8Rng` (never `StdRng`, which is explicitly not reproducible across releases),
`BTreeMap` anywhere a map reaches the output, and no `HashMap` iteration order escaping.
The seed is **never** echoed in the report — it would be a re-identification key.

## 3. Engine v0 — the decisions that keep it honest

### Value-local transforms only
v0 does **not** resample columns from a fitted distribution. Independent per-column resampling
silently decorrelates `city`/`state` and `birth_date`/`age`, shipping incoherent rows and making
requirement #1 a statistical claim rather than an invariant. Instead every transform is local to
the value and preserves the part that carries the distribution:

- **Email** — pseudonymous local part, **domain kept verbatim**.
- **Phone / CEP** — area code and prefix kept, remainder randomized.
- **CPF / CNPJ** — regenerated with valid check digits.
- **FullName** — resampled from a small bundled pt-BR corpus, token count preserved, emitted in Title Case. Replaying source casing would double-count with the `MalformedName` re-injection below.
- **Numeric / Date** — jittered in place within a bounded window.
- **PK / FK** — minted through the ledger (below).
- **FreeText / Unknown** — length-preserving redaction. Conservative by default.

Distribution fidelity then holds **by construction** — the spec's "54% gmail, 21% outlook"
example comes out exact — joint distributions survive for free, and the histogram sampler is
deleted entirely. The distribution histogram is still computed, but for the *report*.

### The ledger, and why it is built single-threaded
Pseudonymizing a key by hashing is not injective: 100k integer PKs hashed into `i32` collide
with near-certainty, and a collision silently merges two parent rows — precisely requirement
#4's failure mode. So keys are **minted**, not hashed: a seeded permutation over the observed
domain with explicit collision retry, preserving the source type and width. PK/FK columns are
restricted to `Int | Text` at validation.

Minting is insertion-order dependent, so the ledger is built **single-threaded in a
deterministic order** (classes sorted by name, values in row order), then **frozen**. The
parallel transform receives `&Ledger`. This is the only way `RunContext` typechecks as one
thing: `&mut` during the build phase, `&` during transform, with stages *returning* their
timings rather than mutating shared state.

The ledger is keyed by **referential equivalence class**, not by `(table, column)`: union-find
over key columns joined by FK edges, one mapping per class. A chain `c.x → b.y → a.z` shares a
single mapping, which is what makes requirement #4 hold transitively. This also dissolves FK
cycles as a special case, so no topological sort and no cycle detection are needed.

### Anomalies are synthesized, never replayed
Detected anomaly exemplars are *production PII*. Re-injection calls a per-kind
`corrupt(value, rng)` that manufactures the same class of corruption on already-anonymized
data. Captured exemplars never reach the output or the report.

Re-injection corrupts the **recorded source positions**, not freshly drawn ones. A row carrying
both a malformed email and a malformed document is one bad data-entry event, and redistributing
the two defects invents two broken rows that never existed; real dumps also cluster malformed
data by import batch, which random placement flattens. Preserving positions also makes the count
exactly `positions.len()`, with no rate round-trip through a float, and removes any competition
between kinds for a shared pool of positions.

### Compliance check: what is enforced vs. what is reported
Distribution fidelity and k-anonymity pull against each other — preserving a rare value
faithfully is exactly what makes a row re-identifiable. v0 declares precedence rather than
pretending to resolve it:

- **Enforced** (failure triggers a retry): **leakage** — no output value of a high-cardinality
  direct-identifier column appears in that column's source value set.
- **Enforced**: **referential** — the output's orphan-FK set is **identical to the input's**.
  Not empty. Dangling FKs are the canonical production bug, and requirement #3 says production
  bugs must survive; an "orphans must be zero" check would fail every realistic run.
- **Reported, never enforced**: **linkage** — observed minimum equivalence-class size on the
  quasi-identifier set, surfaced as `k_observed` with a `warn`. Mutating values to force k
  would distort the distribution the engine exists to preserve.
- **Reported**: **distribution fidelity** — TVD per column. Expected to be 0 given value-local
  transforms; a non-zero value is a bug signal for the reader.

Retries therefore only ever fire for leakage and referential failures. Escalation is
`enum Strength { fn next(self) -> Option<Self> }`, so termination is proven by the type and
`max_retries` is unnecessary. If no attempt passes, the **best-scoring** attempt is returned
alongside a `fail` verdict and reasons — never silently the last one.

## 4. Repository layout

```
placeholder-data-layer/
├── Cargo.toml                  # workspace; MSRV 1.85 (rand 0.10 is edition 2024)
├── AGENTS.md                   # conventions + branch/PR workflow (dormant until MVP ships)
├── README.md  PLAN.md
├── .github/workflows/ci.yml    # fmt, clippy -D warnings, test, build
├── Dockerfile
├── fixtures/                   # JSON datasets shared by tests
├── crates/
│   ├── anonymizer/             # the engine. no tokio, no axum, no I/O.
│   │   ├── src/
│   │   │   ├── lib.rs          # pub fn anonymize / profile
│   │   │   ├── error.rs        # one Error enum for the crate
│   │   │   ├── policy.rs
│   │   │   ├── context.rs      # RunContext — request-scoped
│   │   │   ├── pipeline.rs     # spec steps 2→3→4→5(→4 retry)→6
│   │   │   ├── report.rs       # FidelityReport (BTreeMap-backed)
│   │   │   ├── dataset/
│   │   │   │   ├── mod.rs      # Dataset, column-major ColumnData
│   │   │   │   ├── value.rs    # Null | Bool | Int | Float | Text
│   │   │   │   └── schema.rs   # TableSchema, ColumnSpec, ForeignKey, validation
│   │   │   ├── classify.rs     # SemanticType enum + classify(name, sample)
│   │   │   ├── profile.rs      # ColumnProfile + categorical/numeric distribution
│   │   │   ├── anomaly.rs      # AnomalyKind, detection, corrupt(value, rng)
│   │   │   ├── transform/
│   │   │   │   ├── mod.rs      # Strategy enum + apply; Strength ladder
│   │   │   │   └── ledger.rs   # equivalence classes, minting, injectivity
│   │   │   └── compliance.rs   # checks + Verdict
│   │   └── tests/
│   └── server/
│       ├── src/
│       │   ├── main.rs  config.rs  telemetry.rs
│       │   ├── job.rs          # JobRegistry + WorkerPool + JobState
│       │   └── http/
│       │       ├── mod.rs  router.rs  error.rs
│       │       ├── dto.rs      # wire types; row-major ⇄ column-major at the edge
│       │       └── routes.rs
│       └── tests/
```

Enum dispatch throughout — no `Classifier` trait registry, no `dyn Strategy`. Trait objects
buy nothing without external plugins and cost indirection and ceremony.

## 5. Public API

### HTTP
| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | liveness |
| GET | `/v1/detectors` | semantic types this build recognizes |
| GET | `/v1/jobs` | in-flight jobs — worker-pool observability |
| POST | `/v1/profile` | steps 2–3 only: classify, profile, anomalies. Backs the Front-end's before/after preview. |
| POST | `/v1/anonymize` | the full pipeline, steps 2–6 |

Errors are RFC 9457 `application/problem+json`. A full worker pool yields `429`.

### Request
```jsonc
{
  "dataset": {
    "tables": [{
      "name": "customers",
      "columns": [
        {"name": "id",    "type": "integer", "pk": true},
        {"name": "email", "type": "string",  "nullable": true}
      ],
      "rows": [[1, "ana@gmail.com"], [2, "bad@@x"]]
    }, {
      "name": "orders",
      "columns": [
        {"name": "id",          "type": "integer", "pk": true},
        {"name": "customer_id", "type": "integer",
         "fk": {"table": "customers", "column": "id"}}
      ],
      "rows": [[10, 1], [11, 1], [12, 2]]
    }]
  },
  "options": {
    "seed": null,
    "column_overrides": {"customers.email": "redact"}
  }
}
```

### Response
```jsonc
{
  "run_id": "...",
  "dataset": { "tables": [ /* same shape, anonymized */ ] },
  "report": {
    "verdict": "pass",
    "columns": [{
      "column": "customers.email",
      "semantic_type": "email", "confidence": 0.97,
      "strategy": "pseudonymize_local_part",
      "distribution_before": {"gmail.com": 0.54, "outlook.com": 0.21},
      "distribution_after":  {"gmail.com": 0.54, "outlook.com": 0.21},
      "total_variation_distance": 0.0,
      "anomalies": {"malformed_email": {"before": 0.08, "after": 0.08}}
    }],
    "checks": [
      {"check": "leakage",      "verdict": "pass"},
      {"check": "referential",  "verdict": "pass", "orphans_in": 3, "orphans_out": 3},
      {"check": "linkage",      "verdict": "warn", "k_observed": 1}
    ],
    "timings_ms": {"classify": 3, "profile": 7, "transform": 22, "compliance": 9}
  }
}
```

**The report never contains raw source values.** A compliance artifact that leaks PII defeats
its own purpose.

### Rust
```rust
pub fn anonymize(dataset: Dataset, policy: Policy) -> Result<Outcome, Error>;
pub fn profile(dataset: &Dataset, policy: &Policy) -> Result<DatasetProfile, Error>;
```
Everything else is an implementation detail behind these two.

## 6. Dependencies (versions verified current)

`axum 0.8` · `tokio 1` · `rayon 1.12` · `serde 1` · `serde_json 1` · `thiserror 2` ·
`tracing 0.1` + `tracing-subscriber 0.3` · `rand 0.10` + `rand_chacha 0.10` · `blake3 1.8` ·
`uuid 1` · `tower 0.5` + `tower-http 0.7` · `crossbeam-channel 0.5` · `proptest 1.11` (dev; 1.12 requires Rust 1.88, above our MSRV). New in P2: `regex 1.13`.

Known breakage to code against: axum 0.8 path params are `{id}`, not `:id`, and `#[async_trait]`
is gone. rand 0.10 renames `Rng`→`RngExt` and `thread_rng()`→`rng()`. `blake3::keyed_hash` for
keyed hashing — `sha2 0.11`/`hmac 0.13` sit on the churning `digest 0.11` preview. HTTP tests
use `tower::ServiceExt::oneshot` on the `Router`; `axum-test` has no stable release.

## 7. Cross-cutting

- **Config** — environment, `DL_` prefix: bind address, body limit, rayon threads, worker-pool size, log level.
- **Observability** — `tracing`, one span per pipeline stage, request id propagated, stage timings in the report.
- **Errors** — `thiserror` in the engine, RFC 9457 at the HTTP edge. No `unwrap` outside tests.
- **Lints** — `clippy -D warnings` in CI; `unsafe` forbidden at crate root.

## 8. Testing

**Unit**: leaf correctness only — check-digit validators, TVD math, ledger injectivity, union-find.

**Functional battery**, driven through the public API, each landing with the phase that earns it:

1. **Distribution preserved** — exact equality on the domain histogram (value-local transforms make this an invariant, not a sampling claim).
2. **Cross-table correlation** — the FK partition is isomorphic: multiset of group sizes preserved, *and* injectivity holds — no two source keys collapse to one.
3. **Anomalies reproduced** — exact per-kind count match, input vs output.
4. **Referential integrity** — the output orphan set is identical to the input's.
5. **No leakage** — on a fixture containing a high-cardinality identifier column, zero source values survive.
6. **Determinism** — pinned seed reproduces byte-identical output.
7. **Schema preservation** — types, nullability, column order, row count unchanged.
8. **Edge cases** — empty tables, all-null columns, single row, unicode, self-referencing FK, composite keys, FK cycles.
9. **Parallelism determinism** — same pinned seed under `RAYON_NUM_THREADS=1` and `=8` produces identical output.

**HTTP integration**: happy path, malformed payload, dangling FK reference, oversized body, unknown column override, worker pool saturation → 429.

## 9. Implementation order

Each phase lands its own tests and ends with a review pass; a small refactor is considered
*before* each new slice.

| # | Model | Deliverable |
|---|---|---|
| P0 | Opus | Workspace, pinned deps, `Dataset`/`Value`/`Policy`/`Outcome`/`Error`, `/health`, AGENTS.md, CI, Dockerfile |
| P1 | Sonnet | Schema validation, **key-equivalence classes**, DTOs + column-major conversion, job registry + worker pool, passthrough `/v1/anonymize`. Fixtures + tests #6, #7 as regression guards. |
| P2 | Sonnet | Classification + profiling (step 2), `/v1/profile`, `/v1/detectors`. Test #1. |
| P3 | Sonnet | Anomaly detection + `corrupt` (step 3). Test #3. |
| P4 | Sonnet | Ledger minting, transform strategies, `Strength` ladder (step 4). Tests #2, #8, #9. |
| P5 | Sonnet | Compliance checks, retry loop, fidelity report (steps 5–6). Tests #4, #5. |
| P6 | Sonnet | TypeScript client (§11), GHCR image build and push in CI, README, TODO sweep, final review |

P1 delivers equivalence classes because P4's ledger depends on them. The `Strength` ladder is a
P4 artifact because P5's retry loop consumes it. The report is merely serialization of P2/P3/P5
output, so it rides along with P5 rather than claiming a phase.

## 10. Key identity and the preserve opt-in

### Everything is minted

No key passes through by default, integers included. A CPF stored as `BIGINT` is indistinguishable
from a surrogate id to the classifier — `classify` short-circuits to a numeric type when every
sampled value is numeric, so the document shape checks never run. Brazilian schemas store
documents as integers routinely, so a pass-through default would publish them in the clear.

Minting costs nothing operationally. Row order is a tested contract, so Core — which still holds
the dump it sent — recovers any mapping it needs by positional join. A developer reporting "row
4392 has a broken CPF" is just as actionable against a minted key as a preserved one.

Minting also severs an external handle: with keys preserved, anyone who knows their own id from a
receipt can locate their row, and although the attributes are fake the relational shape is not —
order count, linked entities, time span are real facts about an identifiable person.

### Identity scope per value space

- **UUID-shaped values — global.** The same UUID anywhere in the dataset mints to the same output,
  declared foreign key or not. UUIDs are globally unique by construction, so equal values are the
  same entity; audit and log tables routinely carry them with no FK declared.
- **Integers — per equivalence class.** `users.id = 1` and `products.id = 1` are unrelated. Merging
  integer classes on shared values would collapse every integer key column in the schema into one
  class, so no two rows anywhere could share an id and ids would stop starting at 1 per table.
- **Other text keys — per equivalence class**, with an override when two columns are known to be
  linked without a declared FK.

### The preserve opt-in

A DBA may exempt a column or a whole table — a `payment_type` lookup of `1 Credit, 2 Debit` is
meaningless once minted, and its label is not personal data.

- **Preserving a key column preserves its entire equivalence class.** Preserving one side of a
  foreign key while minting the other would point every referencing row at a value that no longer
  exists. Propagation reaches only key columns, so it widens a link, never an attribute.
- **Preserve is refused on a column classified as a direct identifier** unless explicitly forced,
  naming the column in the error. Otherwise exempting a table quietly publishes every document in
  it. The lookup-table case stays one line; the dangerous case has to be stated twice.
- **The preserve set appears in the fidelity report**, each entry marked as an explicit request or
  as class propagation. The first question an auditor asks of an anonymization report is what was
  not anonymized.

## 11. TypeScript client for Backend Core

Backend Core is Node/TypeScript and currently would have to hand-write its request shapes from
this document. That is where integration bugs come from, so the contract ships as code.

`clients/typescript/` exposes **one class**. Types are **generated** from the Rust DTOs with
`ts-rs` and verified current in CI — hand-written types are correct exactly once, on the day they
are written.

```ts
const dataLayer = await DataLayer.connect("http://data-layer:8080");
const result = await dataLayer.anonymize(payload);
if (result.success) { /* result.dataset, result.report */ }
```

`connect` probes `/health` before resolving, so a misconfigured URL fails at application start
rather than on the first migration. It is a static factory because a JS constructor cannot await.

The result carries `{ success, dataset, report }` and deliberately does **not** echo the input:
the caller’s original PII should fall out of scope at the call site. `report` is the compliance
artifact Backend Core persists for the audit trail.

### Considered and rejected

- **Unix domain socket instead of TCP.** Would remove the network surface for PII and the risk of
  binding `0.0.0.0` by mistake, but every other service in the platform is a container with a
  port, and sharing a socket across containers needs a shared volume. Not worth being the odd one
  out in a compose file four other people run.
- **Spawning the service as a child of Backend Core.** Good local-dev ergonomics, but it needs a
  supervisor (restart, backoff, readiness, zombie reaping, log demuxing), couples the two
  services’ deployment, and makes the Data Layer no longer independently addressable. Process
  supervision is a solved problem that belongs to Docker, not to Node.
- **FFI into Backend Core.** The serialization cost it would save is real, but the process
  boundary is part of the platform’s compliance argument, and a Rust abort would take the
  migration orchestrator down with it. Revisit only if the engine is ever embedded inline — an
  anonymizing read proxy in front of the sandbox would be the case that justifies it.

## 12. Deferred ideas (noted, not built)

Recorded here and as `TODO` comments so later agents inherit the intent:
- Streaming NDJSON transport for dumps that exceed memory.
- Async job endpoint — the registry and worker pool already exist for it.
- Cross-run pseudonym stability, if a DBA ever needs a durable sandbox identity.
- Differential privacy budget per column sensitivity (spec Part 4.1).
- ML-assisted PII detection beyond regex and column-name heuristics (spec Part 4.1).
- Oracle Data Safe as a verification cross-check (spec Part 4.2).
- Fairness between concurrent requests: today a large request can monopolize the rayon pool.
- A `Username` semantic type. Handles are direct identifiers and must be anonymized, but they
  need handle-shaped replacements — lowercase, no spaces, length-similar, often unique-keyed.
  Mapping them onto `FullName` produces person names where a login belongs and breaks both the
  format and any uniqueness constraint.
- **Non-production row detection** (admin, test, seed, QA fixtures). Rows that are structurally
  valid but not real production data: `admin@`, `test@test`, `@mailinator.com`, "Test User",
  `(00) 00000-0000`, textbook-valid CPFs, the seeded `id=1` account. Three reasons it matters:
  such rows poison the fitted distribution, they inflate the data-quality anomaly rates with
  deliberate breakage, and their presence is itself worth reporting to the DBA.

  Model it as **per-row provenance, not an `AnomalyKind`** — every anomaly kind owes a `corrupt`
  arm and a detect↔corrupt round-trip, and there is no way to corrupt a clean value into being an
  admin account. Forcing it into that enum breaks the round-trip test, and the tempting repair is
  to weaken the test rather than fix the modelling.

  Ship it as **report-only** first. Passing suspected test rows through verbatim is attractive —
  QA keeps its known fixtures in the sandbox — but the error is asymmetric: a false negative costs
  nothing, while a false positive releases a real customer unanonymized. Pass-through needs a
  DBA-reviewed allowlist, never a heuristic alone.

  Score **per field, not per row**, and treat disagreement within a row as the signal worth
  escalating. A row that is unanimously test-shaped is safe to be confident about; a row with a
  textbook-fake CPF beside a real-looking email and address is most likely a real person whose
  document was never captured, so someone typed in a placeholder. That row is real PII wearing a
  test-data costume, and it is exactly what pass-through would leak. Surface it:

      7 entries look dev/ops related — "test"/"admin" tokens and known-fake CPFs.
      1 of those pairs a known-fake CPF with a real-looking email and address.
      Flag it for review?
- **A thin `placeholder-platform` repo**, once the services stabilise. It holds only
  `docker-compose.yml`, cross-service integration tests and architecture docs, referencing each
  service as a published GHCR image rather than as source — no git submodules, whose detached
  HEADs and forgotten pointer updates cost a five-person team more than they save. Its real job
  is recording which five service versions are known to work together. A gitignored
  `docker-compose.override.yml` building one service from a sibling checkout keeps local
  iteration from requiring an image push.

  The cost of that topology is that a cross-service contract change becomes two-phase: publish
  an image here, then bump the tag there. Which is the strongest argument for the generated
  TypeScript client in §11 — with types derived from the Rust DTOs, a contract change breaks
  Backend Core at compile time instead of at runtime during integration.
- **The leakage check compares whole values only.** A transform that preserves part of a value
  verbatim — `PreservePrefix`'s area code, by design — leaves that residue outside what the check
  can see; only a cell that survives *entirely* unchanged is caught. A fixed-pattern residue
  detector is not a safe blind fix: it would have to distinguish an intentional verbatim prefix
  from an unintentional one, and getting that distinction wrong either breaks `PreservePrefix` by
  design or still misses the residue by accident. Closing this is calibration work for the team
  tuning the engine, not infrastructure.
- **`ResampleName`'s pool shrinks as `Strength` escalates** (400 → 64 → 9 combinations at
  `Default` → `Aggressive` → `Maximum`), so an escalation retry samples from an increasingly
  cramped space. The retry loop now redraws independently at each strength rather than reusing the
  same stream at a smaller modulus, so escalation can heal a collision instead of only reshuffling
  it — but a smaller pool is still a smaller pool, and a dataset with enough rows will eventually
  make even an independent draw collide more often at `Maximum` than at `Default`. Whether the
  ladder should widen the corpus instead of narrowing it is an engine-tuning question, not one this
  phase answers.
