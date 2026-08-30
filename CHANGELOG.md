# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries from `v0.1.0` onward are generated from commit subjects by
[git-cliff](https://git-cliff.org); see `cliff.toml`.

## [Unreleased]

## [0.0.1] - 2026-08-30

Repository foundation. No functionality beyond a health endpoint.

### Added

- Implementation plan covering scope, module boundaries, the render pipeline
  and its latency budget, the error taxonomy and the milestone breakdown
  (`PLAN.md`).
- Six architecture decision records (`docs/adr/`).
- Pinned toolchain, format, lint, licence and advisory policy.
- axum service with `/healthz` and `/readyz`, JSON logging and graceful
  shutdown.
- CI: fmt, clippy, test on stable and beta, cargo-deny, typos, coverage, and
  release builds for the gnu and musl targets.

[Unreleased]: https://github.com/ibrahimadlani/ibrahim-posters/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/ibrahimadlani/ibrahim-posters/releases/tag/v0.0.1
