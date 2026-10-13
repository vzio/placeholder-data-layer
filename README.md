# Placeholder Data Layer

The anonymization microservice of the Placeholder database-migration platform. It turns a
production dump or sample into an anonymized but statistically faithful dataset for a validation
sandbox: it never reads or writes production itself, only transforms the rows it is handed and
returns the result. `crates/anonymizer` is the deterministic, I/O-free engine; `crates/server`
is the Axum HTTP transport around it. `PLAN.md` is the design of record.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | liveness |
| GET | `/v1/detectors` | semantic types this build recognizes |
| GET | `/v1/jobs` | in-flight jobs — worker-pool observability |
| POST | `/v1/profile` | classify, profile, detect anomalies — no transformation |
| POST | `/v1/anonymize` | the full pipeline: classify, profile, detect, transform, check |

Errors are RFC 9457 `application/problem+json`. A full worker pool returns `429`.

### `POST /v1/anonymize`

Request:

```json
{
  "dataset": {
    "tables": [{
      "name": "customers",
      "columns": [
        {"name": "id", "type": "integer", "pk": true},
        {"name": "email", "type": "string", "nullable": true}
      ],
      "rows": [[1, "ana@gmail.com"], [2, "bruno@gmail.com"]]
    }]
  },
  "options": {"seed": 1}
}
```

Response — real output for the request above (abridged: a real one also reports the `id` column
(`numeric_id` semantic type), each column's `confidence` and `anomalies`, the report's
`preserved_columns`, `accepted_strength` and `attempts`, `timings_ms`, and the `linkage` check):

```json
{
  "run_id": "2994141e-bc73-4772-b4c5-d4cc75d5e816",
  "dataset": {
    "tables": [{
      "name": "customers",
      "columns": [
        {"name": "id", "type": "integer", "nullable": false, "pk": true},
        {"name": "email", "type": "string", "nullable": true, "pk": false}
      ],
      "rows": [[8, "m8xispew07@gmail.com"], [7, "re17hib4t0@gmail.com"]]
    }]
  },
  "report": {
    "verdict": "pass",
    "columns": [{
      "column": "customers.email",
      "semantic_type": "email",
      "strategy": "pseudonymize_local_part",
      "distribution_before": {"gmail.com": 1.0},
      "distribution_after": {"gmail.com": 1.0},
      "total_variation_distance": 0.0
    }],
    "checks": [
      {"check": "leakage", "verdict": "pass", "checked_columns": ["customers.email"]},
      {"check": "referential", "verdict": "pass", "orphans_in": 0, "orphans_out": 0}
    ]
  }
}
```

The report never contains a raw source value or the run seed: it is a compliance artifact meant
to be persisted, and persisting PII would defeat its own purpose.

### `POST /v1/profile`

Same request shape as `/v1/anonymize` — real output for the same request above, before any
transformation (abridged: a real one also profiles the `id` column, `numeric_id` semantic type):

```json
{
  "run_id": "7444ef44-1e4d-47e4-8c1b-a4705d9baa08",
  "profile": {
    "columns": {
      "customers.email": {
        "column": "customers.email",
        "semantic_type": "email",
        "confidence": 1.0,
        "null_rate": 0.0,
        "distinct": 2,
        "distribution": {"kind": "categorical", "top": {"gmail.com": 1.0}, "other": 0.0, "cardinality": 1},
        "anomaly_rates": {}
      }
    }
  }
}
```

## The four spec requirements, and the tests that prove each

1. **Statistical distribution preserved per column** —
   `a_domain_preserved_categorical_column_reports_zero_total_variation_distance` and
   `email_domain_distribution_matches_exact_proportions`
   (`crates/anonymizer/tests/compliance.rs`, `classification_and_profiling.rs`).
2. **Correlations between tables maintained** — `fk_partition_is_isomorphic_across_a_two_hop_chain`
   (`crates/anonymizer/tests/correlation.rs`): the multiset of foreign-key group sizes is
   preserved, and the key mapping is injective.
3. **Real anomalies reproduced** — `anomaly_counts_survive_the_pipeline_exactly`
   (`crates/anonymizer/tests/anomalies.rs`): malformed input survives the pipeline at the same
   per-kind count it went in with, rather than being cleaned up.
4. **Referential integrity intact** — `an_orphan_fk_value_survives_as_an_orphan`
   (`crates/anonymizer/tests/correlation.rs`) and `a_value_level_orphan_fk_survives_at_the_same_position`
   (`crates/anonymizer/tests/compliance.rs`): a dangling foreign key stays dangling, at the same
   position, rather than being silently healed.

Leakage and schema preservation are not spec requirements on their own, but back the four above:
`crates/anonymizer/tests/leakage.rs` and `compliance.rs` establish that no source value a direct
identifier held survives *intact* into the output or the report, and `crates/anonymizer/tests/edge_cases.rs`
and `crates/anonymizer/tests/parallelism.rs` establish that a pinned seed reproduces byte-identical
output under both `RAYON_NUM_THREADS=1` and `=8`.

## Running it

### Cargo

```
cargo run -p server
```

Reads configuration from `DL_`-prefixed environment variables (`DL_BIND_ADDR`, defaulting to
`0.0.0.0:8080`; see `crates/server/src/config.rs` for the rest) and listens on that address.

### Docker

```
docker build -t placeholder-data-layer .
docker run -p 8080:8080 placeholder-data-layer
```

CI publishes the image built from this `Dockerfile` to
`ghcr.io/<owner>/<repo>:latest` (and `:<commit-sha>`) on every push to `main`.

### Tests

```
cargo build --workspace --locked
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
RAYON_NUM_THREADS=1 cargo test --workspace
RAYON_NUM_THREADS=8 cargo test --workspace
```

Both thread counts matter: a pinned seed must reproduce byte-identical output regardless of how
rayon splits the work, and a single-thread run cannot catch a ledger mutated under parallelism.

## TypeScript client

`clients/typescript/` is a generated-types HTTP client for Node (`fetch`, no other runtime
dependency):

```ts
const dataLayer = await DataLayer.connect("http://data-layer:8080");
const result = await dataLayer.anonymize(payload);
if (result.success) { /* result.dataset, result.report */ }
```

See `clients/typescript/src/index.ts` for the rest of the surface. Every request/response type
under `clients/typescript/src/generated/` is generated from the Rust DTOs with
[`ts-rs`](https://github.com/Aleph-Alpha/ts-rs), not hand-written; CI regenerates them
and fails if the checked-in output has drifted. Regenerate locally with:

```
cargo test --workspace --features ts export_bindings
```

**A minted key can exceed what a JS `number` holds losslessly.** `Value::Integer` is an `i64`,
and `mint_key` preserves the input's magnitude, so an 18-19 digit key column mints 18-19 digit
pseudonyms — well past ±(2^53 − 1). A plain `JSON.parse` would round three distinct keys like
`5959995937546829429`, `...430` and `...431` onto the same double, silently breaking the
injectivity the ledger guarantees and the compliance report asserts. The client parses every
response with a reviver that reads `JSON.parse`'s own source text, so an integer literal in that
range survives as the exact digit string instead; `Value`'s generated TypeScript type is
`number | string` for exactly this reason. Compare it as a string, not by coercing to `number`.

## What this PoC engine does not do

This build is infrastructure-first: the HTTP surface, job machinery, determinism guarantees and
report shape are meant to outlive a smarter engine plugged in underneath them, but the engine
itself is deliberately crude. Concretely, today it does not:

- **Detect partial verbatim survival.** The leakage check (`crates/anonymizer/src/compliance.rs`)
  compares whole values only: an output cell is flagged only when it equals a source cell
  outright. A transform that preserves part of a value by design — `PreservePrefix`'s area code —
  leaves that residue outside what the check can see, and an unrelated bug that leaves most of a
  value untouched passes the same way. This has produced a real PII leak in earlier phases (see
  `HANDOFF.md`); closing it is calibration work for whoever tunes the engine next, not
  infrastructure, and the gap is pinned by `a_value_that_survives_in_large_part_is_flagged_as_leaked`,
  an `#[ignore]`d test in `crates/anonymizer/tests/compliance.rs` that encodes the behavior wanted.
- **Detect non-production rows** (admin/test/seed fixtures mixed into real data). Designed in
  `PLAN.md` §12, not built.
- **Distinguish a surrogate key from an ordinary numeric measurement.** Every numeric column
  mints, including a plain age or price stored as an integer — classification cannot tell them
  apart from a CPF stored as `BIGINT`, so nothing numeric is trusted with pass-through.
- **Give handles a handle-shaped replacement.** There is no `Username` semantic type; a login
  column not otherwise classified lands on `FreeText` or `FullName`, neither of which is right
  for a login.
- **Split `MalformedName`'s sub-kinds.** All-caps, all-lowercase and embedded-digit malformed
  names are one `AnomalyKind`, so their relative mix is not preserved across the pipeline even
  though the total count is.
- **Apply differential privacy, use ML-assisted detection, or cross-check against Oracle Data
  Safe.** All out of scope for this PoC per spec Part 4.1/4.2.
- **Stream, or offer an async job endpoint.** The whole dataset is held in memory; the job
  registry and worker pool exist (`GET /v1/jobs`) but every request is still synchronous from the
  caller's point of view.

The full list, with the reasoning behind each, is `PLAN.md` §12 ("Deferred ideas"), cross-referenced
by `grep "TODO: fine tune"` across `crates/` for the engine-specific backlog.
