# Poster Service

HTTP API that composites custom movie posters from TMDB artwork: a background
image, a gradient gaussian blur rising from the bottom edge, a darkening ramp
for legibility, a title logo, and a row of text-driven badges.

Written in Rust. Renders are content-addressed and served with a one-year
`immutable` cache directive, so the CDN does the work in steady state.

> **Status: v0.0.1 — foundation only.** The plan, the decision records and the
> scaffolding are in place. Rendering arrives in milestone M4; see
> [PLAN.md § 12](PLAN.md#12-milestones).

## How it works

```
POST /v1/posters            -> validate, resolve preset + overrides, hash, store spec
                               returns { key, url }
GET  /v1/posters/{key}.webp -> render or serve from cache
                               Cache-Control: public, max-age=31536000, immutable
GET  /v1/presets            -> preset catalogue with resolved defaults
GET  /healthz               -> liveness
GET  /readyz                -> readiness
GET  /metrics               -> Prometheus
```

The split exists because a CDN will not cache a `POST`. The `POST` is cheap —
validation, a hash and one small write, no image work. The `GET` carries an
opaque content-addressed key and is immutably cacheable.

The key is `blake3` over the *resolved and clamped* specification, plus a
`RENDER_VERSION` constant. Two consequences: requests that differ only in ways
the renderer ignores converge on one cache entry, and a change to the renderer
invalidates every derived key mechanically rather than requiring a purge.

## Design targets

| | |
|---|---|
| Render latency | p50 < 80 ms, p99 < 250 ms at w1000 |
| Peak traffic | 1–20 req/s |
| Cache hit rate | > 90 % steady state |
| Default output | 1000×1500 WebP; 2000×3000 opt-in |
| Background source | `image.tmdb.org` only |

The latency figures are *render* time with source bytes in hand. See
[PLAN.md § 14.1](PLAN.md#141-the-latency-target-needs-a-defined-start-point)
for why the end-to-end number on a cold cache is necessarily larger, and what
it actually is.

## Security posture

The API never accepts a URL. It accepts a TMDB `poster_path` matching a strict
pattern and builds the CDN URL server-side, so there is no input that produces
a request to another host — SSRF is eliminated structurally rather than
filtered. Upstream responses are capped at 20 MB by streamed byte count
(`Content-Length` is not trusted) and image dimensions are read from the header
and rejected above 8000 px per side *before* the decoder allocates, because a
40 KB JPEG can legitimately decode to several gigabytes.

`#![forbid(unsafe_code)]` at the crate root. Details in
[PLAN.md § 6](PLAN.md#6-security-model).

## Running it

```sh
cargo run
curl localhost:8080/healthz
```

Configuration is environment-driven; the full table, including which values
are secrets, is in [PLAN.md § 9](PLAN.md#9-configuration). Note that
`TMDB_API_KEY` is **not** required: clients supply `poster_path` directly and
`image.tmdb.org` serves artwork unauthenticated.

## Development

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features
cargo test --doc --all-features
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for branching, commit and documentation
conventions.

## Documentation

| | |
|---|---|
| [PLAN.md](PLAN.md) | Full implementation plan: architecture, types, pipeline, budgets, tests, milestones, risks |
| [docs/adr/](docs/adr/) | Architecture decision records |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Conventions |
| [CHANGELOG.md](CHANGELOG.md) | Release history |

## Licence

[MIT](LICENSE).
