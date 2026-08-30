# Contributing

## Branching

Trunk-based on `main`. Work happens on short-lived branches prefixed `feat/`,
`fix/`, `perf/`, `refactor/`, `test/`, `docs/` or `chore/`, and is
squash-merged. `main` is protected: linear history, required status checks, no
force pushes. See `docs/adr/0006-trunk-based-branching.md`.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/), imperative mood,
subject 72 characters or fewer, lower case, no trailing full stop. One logical
change per commit.

The body carries the reasoning. A subject says what changed; the body says why
that was the right change, what was given up, and what alternative was
rejected. Commits whose rationale is self-evident do not need a body; most are
not that.

Enforced by `commitlint` in CI, on both the individual commits and the pull
request title — merges are squashed, so the title is what lands.

## Documentation

- `#![warn(missing_docs)]`: every public item is documented.
- Rustdoc order: one-line summary, blank line, expanded description, then
  `# Arguments`, `# Returns`, `# Errors`, `# Panics`, `# Examples` where they
  apply.
- Doc examples compile and run under `cargo test`. Use `no_run` only where
  network or filesystem access is genuinely required, not to avoid making an
  example work.
- Module-level `//!` docs state the module's responsibility and its invariants.

## Comments

Comments explain **why**, not what. `// increment counter` above `i += 1` is
noise. Comment non-obvious algorithmic choices, invariants, safety arguments,
performance trade-offs, and links to specifications.

Everything in the repository is in English.

## Before pushing

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features
cargo test --doc --all-features
```

`lefthook install` runs the first two on every commit and the tests on push.

## Changing rendered output

**Any change that alters rendered pixels must bump `RENDER_VERSION` in
`src/lib.rs`.**

Posters are served with a one-year `immutable` cache directive and there is no
invalidation path. Without the bump, the CDN keeps serving posters produced by
the previous renderer, and nothing can correct it. CI fails a visual regression
diff that arrives without a bump — but the gate exists to catch mistakes, not
to be the reason you remember.
