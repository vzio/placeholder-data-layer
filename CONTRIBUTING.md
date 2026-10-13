# Contributing

`README.md` says what this service is and how to run it. `ARCHITECTURE.md` is the design of
record. This document is the rules.

## Branches and pull requests

- **No direct pushes to `main`.** Every change arrives as a pull request, including a one-line
  documentation fix.
- One branch per change, named `feat/<slice>`, `fix/<slice>` or `chore/<slice>`.
- Every pull request needs **CI green** and a **human review**. An agent may open and revise a
  pull request; only a person merges one.
- **Pull requests are squash-merged.** The branch's history is working history and is not kept;
  the squash commit message is the one that lands on `main`, so it must stand on its own.
- A pull request says what changed and why. If it changes behaviour a caller can observe, it says
  that explicitly.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/): `<type>(<scope>): <subject>`.

```
feat(transform): add Username semantic type
fix(compliance): fail the leakage check when a column cannot be evaluated
chore(ci): bump node to 24
docs(architecture): record why linkage is never enforced
```

Types in use: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `chore`, `build`, `ci`. Scope is
the crate or module touched (`anonymizer`, `server`, `transform`, `compliance`, `client`, `ci`).
A breaking change to the HTTP surface or to a generated type gets a `!` after the scope and a
`BREAKING CHANGE:` footer — Backend Core compiles against those types.

Subject lines are imperative and describe the change, not the session that produced it. No
review-finding identifiers, no phase numbers, no dates.

## ARCHITECTURE.md is the design of record

Follow it. Where the code and the document disagree, the document is the bug — fix it.

Changing the architecture is allowed and sometimes right, but it is a deliberate act:

- The change and the `ARCHITECTURE.md` update land in the **same pull request**. A merged change
  that contradicts the document is not finished.
- The reason goes in the document, not only in the pull request. Six months from now the pull
  request is hard to find and the document is not.
- "Load-bearing design decisions" and "Considered and rejected" exist specifically to stop a
  plausible-looking change from quietly undoing a deliberate one. If your change touches
  something listed there, the pull request has to say why the recorded reasoning no longer holds.

Architecture is not changed to make a test pass, to work around a lint, or because an alternative
is more familiar.

## Fine-tuning the engine is test-first

The infrastructure is finished; the engine is deliberately crude. Making it smarter is the bulk
of the work ahead, and it is done in this order:

1. **Write the test first.** Name the behaviour you want the engine to catch and assert it
   directly, as a functional test through the public API.
2. **Then improve the engine** until the test passes.
3. **If it does not pass, the test stays.** Mark it `#[ignore]` with a reason starting
   `TODO: fine tune to catch this`, and leave it in the suite.

```rust
#[test]
#[ignore = "TODO: fine tune to catch this — partial verbatim survival is not detected; whole-value comparison only"]
fn a_value_that_survives_in_large_part_is_flagged_as_leaked() {
```

**Never delete a test because it fails, and never weaken its assertion to make it pass.** An
ignored test that fails under `--ignored` is the most valuable artifact in the repository: it
states the contract wanted, proves the gap is real, and turns into a passing test the moment
someone closes it. Deleting it throws away the only precise record of what is missing, and
weakening it is worse — the suite then asserts something nobody wants and looks green doing it.

An ignored test earns its place by being **specific**. "The engine should be smarter" is not a
test. Each one names a single behaviour, with a fixture that exhibits it.

Correspondingly: an ignored test is not a parking space for ideas. Write one when you have hit
an actual gap in actual work, not to enumerate everything a finished engine might do.

Each gap also gets a `TODO: fine tune` comment at the line it applies to, so
`grep "TODO: fine tune" crates/` is the engine backlog.

## Code

- **Comments are a last resort.** Document through naming: a well-named type, function or variant
  replaces the comment that would have explained it. Write a comment only for what the code
  cannot say — a non-obvious invariant, or a decision whose alternative looks more attractive
  than it is.
- **Rustdoc is timeless.** It describes what the item is and the contract it upholds, as if it
  had always been that way. No changelogs, no "fixed the collision bug", no dates, no references
  to prior behaviour or to review feedback. If a reader a year from now would not need the
  sentence, delete it.
- **Comments never cite a review-finding identifier.** A label like `C1` or `M6` is local to the
  throwaway report that assigned it and means nothing outside that document. State the invariant
  the finding pinned, not the finding itself.
- **No `unwrap`, `expect` or `panic!` outside `#[cfg(test)]`.** Errors are `thiserror` enums in
  the engine and RFC 9457 problems at the HTTP edge. An `unwrap` in a handler is a 500 waiting
  for a fuzzer. The one narrow exception: `expect` on a compile-time-constant input that request
  data cannot influence, such as `Regex::new` over a fixed literal pattern — it can only fail for
  a bug a single test run catches, never for anything a caller sends.
- **`clippy -D warnings` is the floor**, with `--all-targets`. Do not silence a lint with
  `#[allow]` where a two-line fix exists; when an allow is genuinely right, scope it to the item.
- **`unsafe` is forbidden** at both crate roots and stays that way.
- **Enum dispatch, never trait objects.** `Strategy`, `SemanticType`, `AnomalyKind` and `Check`
  are enums with match arms. There is no plugin boundary here, so `dyn` buys nothing and costs
  indirection, allocation and a registry to keep in sync. Adding a detector is a variant plus an
  arm, and the compiler finds every site that must change.

### The engine crate is pure

`crates/anonymizer` depends on no tokio, no axum, no filesystem, no clock, no network, no
environment. It takes a `Dataset` and returns an `Outcome`. Every piece of I/O, every wire type
and every async construct lives in `crates/server`. This is what makes the engine testable
without a runtime and parallelisable without a reactor; keep the boundary absolute.

Row-major is the wire shape; column-major is the engine shape. The pivot happens exactly once,
in `http::dto`.

### Determinism is a contract

A pinned `options.seed` must reproduce **byte-identical** output, including under a different
`RAYON_NUM_THREADS`. That property is a test, not an aspiration.

- `ChaCha8Rng` only. `StdRng` is explicitly not reproducible across releases.
- Derive per-stage and per-column substreams from the run seed with `blake3::keyed_hash`, so a
  value never depends on how work was scheduled.
- `BTreeMap` anywhere a map reaches the output or the report. No `HashMap` iteration order may
  escape into results — use one internally only where its iteration order cannot be observed.
- Order-dependent construction (the ledger) runs single-threaded in a deterministic order, then
  freezes. The parallel phase sees `&Ledger`.
- The run seed is never echoed in the report: it is a re-identification key.
- The report never contains a raw source value.

### Tests

Unit tests cover leaf correctness only — check digits, TVD, union-find, ledger injectivity — and
live beside the code in `#[cfg(test)] mod tests`. Everything else is a functional test through
the public API, in `crates/*/tests`, with fixtures in `fixtures/`. HTTP tests drive the `Router`
through `tower::ServiceExt::oneshot`; no network, no ports.

## Before opening a pull request

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
RAYON_NUM_THREADS=1 cargo test --workspace
RAYON_NUM_THREADS=8 cargo test --workspace
```

Both thread counts matter and neither substitutes for the other: a test process can only set
rayon's global pool size once, at construction, so the determinism contract is only established
by running the full suite under each.

If you changed any type carrying `#[cfg_attr(feature = "ts", derive(ts_rs::TS))]`, regenerate the
TypeScript bindings and commit the result — CI regenerates from a clean checkout and fails on any
diff:

```sh
rm -rf clients/typescript/src/generated
cargo test --workspace --features ts export_bindings
```

The TypeScript client supports Node `>=18` as declared in its `package.json`. Anything relying on
a newer runtime feature must be polyfilled or the declared floor raised — **never left to fail
open**. A data-integrity guarantee that silently degrades on an older runtime is worse than no
guarantee, because the caller cannot tell which behaviour it got.
