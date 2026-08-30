# Poster Service

HTTP API that composites custom movie posters from TMDB artwork: a background
image, a gradient gaussian blur rising from the bottom edge, a darkening ramp
for legibility, a title logo, and a row of text-driven badges.

Written in Rust. Renders are content-addressed and served with a one-year
`immutable` cache directive, so a CDN does the work in steady state.

## Using it

```sh
# Describe a poster. Cheap: validation, a hash, one small write.
curl -X POST localhost:8080/v1/posters \
  -H 'content-type: application/json' \
  -d '{
    "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
    "preset": "cinematic",
    "logo":   "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png",
    "badges": [
      { "text": "#17 IMDb",      "style": "accent"  },
      { "text": "Oscar Nominee", "style": "outline" }
    ]
  }'
# { "key": "c44187c7...", "url": "https://…/v1/posters/c44187c7….webp" }

# Fetch it. Immutably cacheable.
curl -o poster.webp localhost:8080/v1/posters/c44187c7….webp
```

| Endpoint | |
|---|---|
| `POST /v1/posters` | Validate, resolve, hash, store the specification |
| `GET /v1/posters/{key}.webp` | Render or serve from cache |
| `GET /v1/presets` | Preset catalogue with resolved defaults |
| `GET /healthz` `/readyz` | Liveness, readiness |
| `GET /metrics` | Prometheus |

The split exists because a CDN will not cache a `POST`, and above a 90 % hit
rate the CDN is doing most of the work.

The key is `blake3` over the *resolved and clamped* specification plus a
`RENDER_VERSION` constant. Two consequences: requests differing only in ways
the renderer ignores converge on one cache entry, and a renderer change
invalidates every derived key mechanically rather than needing a purge.

## Presets

`standard`, `cinematic`, `minimal`, `poster_wall`. Each sets the blur band
height and sigma, the darkening strength, logo geometry and badge height; any
of those can be overridden per request and is clamped after the merge.

## Measured performance

| | Target | Measured |
|---|---|---|
| Render, w1000 | — | 33 ms |
| Encode, w1000 | — | 30 ms |
| **Total, cold** | p50 < 80 ms | **≈ 63 ms** |
| L2 cache hit | — | **0.6 ms** |

Measured on Apple silicon with synthetic fixtures; see
[PLAN.md § 5](PLAN.md#5-render-pipeline) for the per-stage breakdown and
[§ 14.1](PLAN.md#141-the-latency-target-needs-a-defined-start-point) for what
the clock does and does not include.

## Security posture

The API never accepts a URL. It accepts a TMDB `poster_path` matching a strict
pattern and builds the CDN URL server-side, so no input can produce a request
to another host — SSRF is eliminated structurally rather than filtered.
Redirects are refused, which closes the other half.

Upstream responses are capped at 20 MB by streamed byte count, since a chunked
response declares no length at all. Image dimensions are read from the file
header and rejected above 8000 px per side *before* the decoder allocates: a
24-byte PNG header can declare a 14 GB decode target, which no byte cap
catches.

`#![forbid(unsafe_code)]` at the crate root. Details in
[PLAN.md § 6](PLAN.md#6-security-model).

## Running it

```sh
cargo run                       # in-memory storage, public TMDB CDN
curl localhost:8080/healthz
```

Or from a container — 7 MB, `scratch`, non-root:

```sh
docker build -f Dockerfile.musl -t poster-service .
docker run -p 8080:8080 -e POSTER_BIND_ADDR=0.0.0.0:8080 poster-service
```

Configuration is `POSTER_`-prefixed environment variables; the full table is in
[PLAN.md § 9](PLAN.md#9-configuration). `POSTER_TMDB_API_KEY` is **not**
required — clients supply `poster_path` directly and `image.tmdb.org` serves
artwork unauthenticated.

See [docs/deployment.md](docs/deployment.md) for storage lifecycle rules, CDN
setup, capacity sizing and how to ship a renderer change.

## Development

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features
cargo test --doc --all-features
```

251 tests. The ones worth knowing about:

- **Visual regression** renders fixed fixtures and compares against committed
  references under `tests/visual/reference/v{RENDER_VERSION}/`. Comparison is
  tiled, because a whole-image mean misses a moved element — measured at 0.97
  against a limit that has to absorb SIMD noise, where the tiled check scores
  57. Verified against five deliberately broken renderers.
- **Module boundary** fails the build if `src/render/` acquires an async or
  I/O dependency.
- **Property tests** assert the canonical cache-key encoding is injective,
  which is the falsifiable form of "distinct specifications must not collide".

Any change that alters rendered pixels must bump `RENDER_VERSION` in
`src/lib.rs`. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Documentation

| | |
|---|---|
| [PLAN.md](PLAN.md) | Architecture, types, pipeline, budgets, tests, risks, open questions |
| [docs/adr/](docs/adr/) | Architecture decision records |
| [docs/deployment.md](docs/deployment.md) | Operating the service |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Conventions |
| [CHANGELOG.md](CHANGELOG.md) | Release history |

## Licence

[MIT](LICENSE). Bundled fonts are [Inter](https://rsms.me/inter/) under
OFL-1.1; see `assets/fonts/Inter-LICENSE.txt`.
