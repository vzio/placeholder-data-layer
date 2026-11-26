# The target corpus

Each directory here is a scenario: a realistic dataset and the behaviour the engine is wanted to
have on it **once it is ready**. It is a goal definition, not a regression suite: many
expectations are unmet today, and that is the point of writing them down. `ARCHITECTURE.md`
("The target corpus") records why it is built this way; `CONTRIBUTING.md` holds the rules for
changing it. This file is the authoring guide.

```sh
cargo run -p corpus -- scoreboard --verbose     # what is met, what is not, and why
cargo test -p corpus                            # met expectations and fixture checks
cargo test -p corpus -- --ignored               # every unmet target, failing
```

## A scenario directory

| File | Holds |
|---|---|
| `README.md` | What the data is, what mess it carries, and what the scenario is about. |
| `expected.json` | The dataset source, the engine runs, the facts and the expectations. |
| `dataset.json` | A curated, committed dataset in the wire format of `POST /v1/anonymize`. |
| `preview.json` | For a generated dataset: its first rows per table, committed for review. |

A scenario has either `dataset.json` or `preview.json`. A generated dataset is never committed:
its generator regenerates it at test time, and `fixture_is_well_formed` checks the bytes against
the committed `digest` and `preview.json`. Synthetic data only, never a real person: provider
email domains are real, every local part, name, document and phone is invented.

## `expected.json`

```json
{
  "format": 1,
  "scenario": "<directory name>",
  "pack": "seed | commerce | people | legacy",
  "tier": "ci | extended",
  "summary": "one paragraph",
  "dataset": { "source": "file", "path": "dataset.json" },
  "runs": [{ "id": "default", "seed": 7 }],
  "facts": [],
  "expectations": []
}
```

A generated dataset instead says
`{"source": "generator", "name": "...", "seed": 1, "params": {...}, "digest": "blake3:...", "preview_rows": 25}`.
`cargo run -p corpus -- digest <scenario>` and `-- preview <scenario> > preview.json` print what
to commit. Every object rejects unknown fields.

**Runs.** `{"id", "seed", "mode": "anonymize" | "profile", "preserve": [...],
"column_overrides": {"table.column": "strategy"}, "force_preserve": false}`; only `id` and `seed`
are required. A seed is never changed once committed. Each run executes once per test process
and every expectation naming it shares the result.

**Expectations.**

```json
{
  "id": "cpf_is_classified_cpf",
  "status": "met | target",
  "ruling": "obvious | needs_owner | owner_ruled",
  "rationale": "required unless the ruling is obvious",
  "gap": "optional: the ignore reason when the id does not say it",
  "run": "default",
  "assert": { "kind": "semantic_type", "column": "clientes.cpf", "is": "cpf" }
}
```

The id is the test name: unique, snake_case, with at least one underscore, stating the behaviour.
Write every expectation as the behaviour wanted, with `status: target`; run the scoreboard;
promote to `met` only what holds, after checking it holds for the right reason (the scoreboard
prints how many cells, rows or fields each inspected). A tolerance is a ruling, never a knob to
make something pass. Flag a product decision nobody has taken with `needs_owner` and say what
the decision is in `rationale`.

## Standing rulings

The owner has ruled on the decisions that recur across scenarios. A new scenario follows these
defaults without flagging `needs_owner`, records `ruling: owner_ruled`, and states in `rationale`
which default applies and why. `needs_owner` remains for a product decision these defaults do not
cover.

- **Coarse categories pass through with their exact mix.** A state, a city, a blood type, a bank
  code, a vehicle year, a special-needs category, a time of day: many people share each value.
- **Money keeps its mean within two percent.** A heavy-tailed column (payouts, transaction
  amounts) also keeps its median and 90th percentile within five percent (`p50_rel_tol: 0.05`,
  `p90_rel_tol: 0.05`). A mean tolerance a scenario already states stays as written.
- **Counters are unchanged.** Points, quantities, star ratings, order counts and absences use
  `values_unchanged`.
- **Public or business reference data is kept without an explicit preserve.** Job titles, a
  product catalogue and its SKUs, plan prices, legacy codes, 0800 lines, a store's CNPJ, ledger
  text from a closed set, intercom extensions, state lookup tables, product launch dates.
- **A code that looks a person up in an outside system is replaced.** Tracking codes, NF-e access
  keys, police report numbers, invoice numbers, a doctor's CRM, the last four digits of a card. An
  internal ticket protocol number is replaced too, consistently: equal in, equal out; distinct
  in, distinct out.
- **Pseudonyms are consistent.** The same input always maps to the same output: duplicate
  customers and leads, one agent, regular visitors, returning customers, PIX keys, shared test
  CPFs and phones.
- **Names are strict.** No surname survives anywhere, whether a child's, a minor's or a
  customer's. A company name becomes a company name, never a person's name.
- **Bad data stays bad.** An invalid CNPJ stays invalid, the malformed rate is kept, and a PIS
  cell holding a CPF stays a valid CPF. A `0` or any other sentinel is an absent value, not a
  malformed document or a test row; a sentinel email such as `N/A` does not make a placeholder
  row. Repeated spreadsheet header rows are kept as they are.
- **Free text.** Clinical notes are redacted whole. Notes drawn from a closed set of categories,
  and closed code sets such as CID-10, keep that set and stay attached to their row. A column
  mixing document kinds (PIX keys; RG beside CPF) gets a type per value. A street address gets a
  dedicated type.

## Assertion vocabulary

Columns are `table.column`. An assertion that inspects nothing is *unevaluable*: it fails a `met`
expectation and leaves a target unmet. Engine output is read as JSON, so a type or strategy is
named by its wire spelling (`cpf`, `full_name`, `regenerate_document`) and a target may name one
the engine does not have (`address`).

| Family | Kind | Fields | Holds when |
|---|---|---|---|
| Outcome | `verdict` | `is` | The report's verdict is `is`. |
| | `refused` | `error`, `column?` | The run is refused with that `Error` variant, naming `column`. |
| | `accepted_at_strength_at_most` | `strength` | The accepted strength is at most `strength`. |
| | `deterministic` | `threads` | Rerunning in a pool of each thread count gives byte-identical output and report (timings aside). |
| Classification | `semantic_type` | `column`, `is`, `source?` | The report and/or profile (`report`, `profile`, `both` by default) type the column `is`. |
| | `semantic_type_one_of` | `column`, `any_of` | The report's type is one of `any_of`. |
| | `not_semantic_type` | `column`, `is_not` | The report's type is not `is_not`. |
| | `strategy` | `column`, `is` | The report's strategy is `is`. |
| | `profile_agrees_with_report` | | Every column has the same type in both. |
| Privacy | `no_source_survival` | `columns`, `sources?`, `scope`, `whole`, `fragments?` | No output cell equals (`whole`) or keeps a fragment (`{"letters": n, "digits": n}`) of a source cell: its own row's (`row`) or any row's (`column`, digits only). Case-folded; digits compared with punctuation dropped; an email's domain is never a fragment. |
| | `report_free_of_source_values` | `columns`, `min_len` | No source value of at least `min_len` characters appears in the serialised report. |
| | `values_changed` | `column`, `min_fraction` | At least that share of non-null cells changed. |
| Format | `format_preserved` | `column`, `shapes` | A row whose source is one of `shapes` comes back in that shape. |
| | `check_digit_validity_preserved` | `column`, `document` | Valid `cpf`/`cnpj` stay valid, invalid stay invalid. |
| | `null_positions_preserved` | `column` | Nulls stay exactly where they were. |
| Structure | `row_count_preserved` | `tables` (list or `"*"`) | Every table keeps its row count. |
| | `referential_integrity` | `column`, `orphans` | The source has `orphans` dangling references, they stay at the same rows, and every other row still joins the row it joined (positional join). |
| | `key_identity` | `columns` | Equal source keys map to equal output keys and distinct to distinct, across the columns. |
| | `same_source_same_output` | `column`, `normalize` | Rows equal after `none`/`digits`/`casefold` stay equal. |
| | `values_unchanged` | `column` | Every cell is unchanged. |
| Fidelity | `distribution` | `column`, `feature`, `max_tvd` | Total variation distance of the feature (`value`, `email_domain`, `phone_ddd`, `cep_prefix3`, `length`, `shape`, `null`) is at most `max_tvd`. |
| | `distinct_ratio` | `column`, `min`, `max` | Distinct values out over distinct values in lies in `[min, max]`. |
| | `numeric_summary` | `column`, `mean_rel_tol`, `p50_rel_tol?`, `p90_rel_tol?` | Mean and quantiles move by at most that relative error. |
| Bookkeeping | `anomaly_rate` | `column`, `anomaly`, `before: {value, tol}`, `after: "equals_before" \| {value, tol}` | The report's per-row rate of that anomaly kind is as stated. |
| | `non_production_rows` | `table`, `flagged`, `mixed`, `source` | The report or profile counts match (`{"equals": n}` or `{"min": n, "max": n}`). |
| | `preserved` | `column`, `reason` | The report records the column preserved as `requested` or `propagated`. |

**Shapes** are named — `cpf_masked`, `cpf_digits`, `cnpj_masked`, `cnpj_digits`, `cep_masked`,
`cep_digits`, `phone_br_masked`, `phone_br_digits`, `phone_br_e164`, `email`, `date_iso`,
`date_br`, `uuid` — or a regular expression, `{"regex": "\\d{12,13}"}`. A shape matches a whole
value; `contains_count` looks for it inside one.

## Facts

Facts are statements about the source dataset alone, checked by `fixture_is_well_formed`. They
pin what the fixture holds, so an expectation's numbers can be traced to the data and a generator
change that moves them fails loudly: `row_count {table}`, `null_count`, `shape_count {shape}`,
`contains_count {shape}`, `valid_document_count {document}`, `orphan_count`, `distinct_count`,
each with `column` and `equals`.

## Generators

A generator is `fn(seed: u64, params: &serde_json::Value) -> Result<DatasetDto, GenError>`,
registered by its pack in `crates/corpus/src/packs/<pack>.rs`, reading its own parameter struct
with `deny_unknown_fields`. It draws from `generate::rng::Rng` (SplitMix64) in a fixed order on one
thread. The shared building blocks are in `crates/corpus/src/generate/`: coherent people and
companies, valid and invalid CPFs and CNPJs, and the distortions of `mess.rs`. The word lists in
`generate/vocab.rs` are frozen; a pack that needs more keeps its own.
