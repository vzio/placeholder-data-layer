# Placeholder Data Layer

The anonymization microservice of the Placeholder database-migration platform.

It turns a production dump or sample into an anonymized but **statistically faithful** dataset
for a validation sandbox. It never reads or writes production itself: it receives rows over
HTTP, transforms them, and returns the result along with a compliance report. Nothing is
persisted — no database, no disk, nothing retained past the response that produced it.

"Statistically faithful" is the hard part, and it means four things:

1. **Each column keeps its distribution.** The sandbox should be as skewed, as sparse and as
   lopsided as production is.
2. **Correlations between tables survive.** A customer with twelve orders still has twelve.
3. **Real anomalies are reproduced.** Production bugs — malformed phone numbers, impossible
   dates, names in all caps — reach the sandbox instead of being cleaned up on the way. Testing
   a migration against unrealistically tidy data is testing the wrong migration.
4. **Referential integrity holds.** The same source value maps to the same anonymized value in
   every table it appears in, so foreign keys still join.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/health` | liveness |
| GET | `/v1/detectors` | semantic types this build recognizes |
| GET | `/v1/jobs` | in-flight jobs — worker-pool observability |
| POST | `/v1/profile` | classify, profile, detect anomalies — no transformation |
| POST | `/v1/anonymize` | the full pipeline: classify, profile, detect, transform, check |

Send a dataset as tables of columns and rows; get back the anonymized dataset and a report
saying what was done to each column, how far the distribution moved, and whether the compliance
checks passed. Errors are RFC 9457 `application/problem+json`; a full worker pool returns `429`.

The report never contains a raw source value or the run seed. It is meant to be kept, and
keeping PII would defeat its own purpose.

Exact request and response shapes are the generated TypeScript types in
`clients/typescript/src/generated/`, and the DTOs they come from in
`crates/server/src/http/dto.rs`.

## Running it

```sh
cargo run -p server
```

Listens on `0.0.0.0:8080` by default. Configuration comes from `DL_`-prefixed environment
variables — `DL_BIND_ADDR`, `DL_BODY_LIMIT_BYTES`, `DL_WORKER_THREADS`, `DL_LOG_LEVEL`; see
`crates/server/src/config.rs` for the full set and their defaults.

### Docker

```sh
docker build -t placeholder-data-layer .
docker run -p 8080:8080 placeholder-data-layer
```

CI publishes this image to `ghcr.io/<owner>/<repo>:latest` and `:<commit-sha>` on every push to
`main`.

### Tests

```sh
cargo build --workspace --locked
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
RAYON_NUM_THREADS=1 cargo test --workspace
RAYON_NUM_THREADS=8 cargo test --workspace
```

Both thread counts matter: a pinned seed must reproduce byte-identical output regardless of how
the work is split across threads, and a single-threaded run cannot catch a ledger mutated under
parallelism.

## Calling it from Node

`clients/typescript/` is an HTTP client with no runtime dependency beyond `fetch`. Its
request and response types are generated from the Rust DTOs, so a contract change here breaks
the caller at compile time rather than at runtime.

```ts
const dataLayer = await DataLayer.connect("http://data-layer:8080");
const result = await dataLayer.anonymize(payload);
if (result.success) { /* result.dataset, result.report */ }
```

One thing to know when reading cell values: an anonymized key can exceed what a JavaScript
`number` holds exactly, so the client returns integers beyond ±(2^53 − 1) as digit strings and
types them `number | string`. Compare those as strings; coercing to `number` reintroduces the
rounding the client went out of its way to avoid.

## Reading further

| Document | For |
|---|---|
| `ARCHITECTURE.md` | How it works inside, and why it is built this way. Read before changing anything. |
| `CONTRIBUTING.md` | Branch and pull-request workflow, commit format, code conventions, how engine fine-tuning is done. |

The engine is deliberately crude: this build invests in the HTTP surface, the job machinery, the
determinism guarantees and the report shape, so that a smarter engine can be built underneath
them without any of that changing. `ARCHITECTURE.md` lists what it does not yet do, and
`grep "TODO: fine tune" crates/` is the backlog inline in the code.
