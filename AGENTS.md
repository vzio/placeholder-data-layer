# Conventions

`PLAN.md` is the design of record. Read it before touching anything; it is not
edited by implementation work. Phases and their owners are in PLAN.md §9.

## Code

- **Comments are a last resort.** Document through naming: a well-named type,
  function or variant replaces the comment that would have explained it. Write
  a comment only for what the code cannot say — a non-obvious invariant, or a
  decision whose alternative looks more attractive than it is.
- **Rustdoc is timeless.** It describes what the item is and the contract it
  upholds, as if it had always been that way. No changelogs, no "fixed the
  collision bug", no "new in P3", no dates, no references to prior behaviour or
  to review feedback. If a reader a year from now would not need the sentence,
  delete it.
- **Comments never cite a review-finding identifier.** A label like `C1` or
  `M6` is local to the throwaway report that assigned it and means nothing
  outside that document. State the invariant the finding pinned, not the
  finding itself or what the code used to do wrong before it was addressed.
- **No `unwrap`, `expect` or `panic!` outside `#[cfg(test)]`.** Errors are
  `thiserror` enums in the engine and RFC 9457 problems at the HTTP edge. An
  `unwrap` in a handler is a 500 waiting for a fuzzer. The one narrow
  exception: `expect` on a compile-time-constant input that request data
  cannot influence, such as a `Regex::new` call over a fixed literal pattern.
  The pattern itself can only fail to compile for a bug a single test run
  catches, never for anything a caller sends.
- **`clippy -D warnings` is the floor**, with `--all-targets`. CI enforces it
  alongside `cargo fmt --check`. Do not silence a lint with `#[allow]` where a
  two-line fix exists; when an allow is genuinely right, scope it to the item.
- **`unsafe` is forbidden** at both crate roots and stays that way.
- **Enum dispatch, never trait objects.** `Strategy`, `SemanticType`,
  `AnomalyKind` and `Check` are enums with match arms. There is no plugin
  boundary here, so `dyn` buys nothing and costs indirection, allocation and a
  registry to keep in sync. Adding a detector is a variant plus an arm, and the
  compiler finds every site that must change.
- **Stubs carry `_`-prefixed parameters and a `todo!("<phase>: …")` body.**
  Dropping the underscore is part of implementing the function. A `todo!()`
  left standing at the end of a phase is a bug in that phase.

## The engine crate is pure

`crates/anonymizer` depends on no tokio, no axum, no filesystem, no clock, no
network, no environment. It takes a `Dataset` and returns an `Outcome`. Every
piece of I/O, every wire type and every async construct lives in
`crates/server`. This is what makes the engine testable without a runtime and
parallelisable without a reactor; keep the boundary absolute.

Row-major is the wire shape; column-major is the engine shape. The pivot
happens exactly once, in `http::dto`.

## Determinism is a contract, not a preference

A pinned `options.seed` must reproduce **byte-identical** output, including
under a different `RAYON_NUM_THREADS`. That property is a test, not an
aspiration.

- `ChaCha8Rng` only. `StdRng` is explicitly not reproducible across releases.
- Derive per-stage and per-column substreams from the run seed with
  `blake3::keyed_hash` so a value never depends on how work was scheduled.
- `BTreeMap` anywhere a map reaches the output or the report. No `HashMap`
  iteration order may escape into results — use one internally only when the
  order of its iteration cannot be observed.
- Order-dependent construction (the ledger) runs single-threaded in a
  deterministic order, then freezes. The parallel phase sees `&Ledger`.
- The run seed is never echoed in the report: it is a re-identification key.
- The report never contains raw source values.

## Tests

Unit tests cover leaf correctness only — check digits, TVD, union-find, ledger
injectivity — and live beside the code in `#[cfg(test)] mod tests`. Everything
else is a functional test through the public API, in `crates/*/tests`, with
fixtures in `fixtures/`. HTTP tests drive the `Router` through
`tower::ServiceExt::oneshot`; no network, no ports.

Each phase lands the tests listed for it in PLAN.md §8 and ends with a review
pass. Consider a small refactor before starting a new slice, not after.

## Branches and pull requests — not yet active

While the MVP is being built, work lands directly on `main`.

Once the MVP ships, this becomes the workflow:

- One branch per slice, named `feat/<slice>`, `fix/<slice>` or `chore/<slice>`.
- No direct pushes to `main`.
- Every change arrives as a pull request with CI green, opened for **human
  review**, and merged by a human.
- A pull request describes what changed and why, and points at the PLAN.md
  section it serves.

Do not adopt this workflow early, and do not create branches or pull requests
until this section says it is active.
