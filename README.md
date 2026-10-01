# holdout-safe

`holdout-safe` creates deterministic, group-aware folds from JSON Lines data.
Every record with the same entity key stays in one fold, preventing a patient,
device, customer, or experiment from leaking across train and evaluation sets.
An optional label field is balanced with a deterministic greedy strategy, and
the generated manifest can later prove that the exact input still produces the
same split.

It is a local data-preparation tool. It does not claim that a dataset is fair,
representative, statistically powered, or suitable for clinical decisions.

## Install

Stable Rust 1.85 or newer is required.

```bash
cargo install --git https://github.com/nicholas-ruest/holdout-safe
```

Or build from a clone:

```bash
cargo build --release
```

## Quick start

Inspect a plan without writing files:

```bash
cargo run -- plan \
  --input examples/patients.jsonl \
  --group-field patient.id \
  --label-field diagnosis \
  --folds 3 \
  --seed demo \
  --pretty
```

Write `fold-000.jsonl`, `fold-001.jsonl`, and so on, plus `manifest.json`:

```bash
cargo run -- split \
  --input examples/patients.jsonl \
  --group-field patient.id \
  --label-field diagnosis \
  --folds 3 \
  --seed demo \
  --out-dir folds
```

Audit the original input against that manifest:

```bash
cargo run -- audit \
  --input examples/patients.jsonl \
  --manifest folds/manifest.json
```

`audit` exits `0` for a valid reproduction, `2` for a mismatch, and `1` for an
operational or input error. Add `--json` for machine-readable audit output.

## Input rules

- Input must be UTF-8 JSON Lines, with one object per non-empty line.
- `--group-field` and `--label-field` accept dotted object paths.
- Group and label values must be non-null strings, numbers, or booleans.
- Group keys are type-tagged, so the string `"1"` and number `1` are distinct.
- `split` only writes into a new or empty directory and never overwrites files.

The input SHA-256 covers the original bytes, while assignments are derived from
parsed scalar keys, the requested fold count, and the seed. Groups are placed
largest-first; each placement minimizes the normalized increase in record and
per-label imbalance. Hash ordering and explicit tie-breakers keep the result
reproducible.

## Verification

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo package --locked
```

The crate forbids unsafe Rust. CI repeats formatting, linting, and tests on the
stable toolchain.

## License

MIT
