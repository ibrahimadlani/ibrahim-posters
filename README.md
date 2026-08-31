# Poster Service

HTTP API that composites custom movie posters from TMDB artwork: a background
image, a gradient gaussian blur rising from the bottom edge, a darkening ramp
for legibility, a title logo, and a row of text-driven badges.

Written in Rust. Renders are content-addressed and served with a one-year
`immutable` cache directive, so a CDN does the work in steady state.

## Using it

Name a film or series by its TMDB identifier. The service resolves the artwork
itself.

```sh
curl -X POST localhost:8080/v1/posters \
  -H 'content-type: application/json' \
  -d '{
    "tmdb_movie_id": 27205,
    "preset": "cinematic",
    "badges": [
      { "text": "#13 IMDb", "style": "accent"  },
      { "text": "Nolan",    "style": "outline" }
    ]
  }'
# { "key": "0d31a4e0…", "url": "https://…/v1/posters/0d31a4e0….webp" }

curl -o poster.webp localhost:8080/v1/posters/0d31a4e0….webp
```

Series work the same way with `"tmdb_tv_id"`.

### Choosing the artwork

By default the service picks: preferred language first, then
language-neutral, then anything else; within a band, highest rated, with votes
breaking a tie. TMDB's editorially primary poster leads regardless.

To choose yourself, browse what a title offers — the list is in the same order,
so its first entry is exactly what `auto` selects:

```sh
curl localhost:8080/v1/artwork/movie/27205
```
```json
{ "kind": "movie", "id": 27205,
  "posters": [ { "path": "/xlaY2zyz….jpg", "language": "en",
                 "vote_average": 8.03, "width": 2000, "height": 3000 } ],
  "logos":   [ { "path": "/iXYh7y0v….png", "language": "en",
                 "vote_average": 6.72, "width": 4317, "height": 461 } ] }
```

Then name one:

```json
{ "tmdb_movie_id": 27205, "logo": "/eS5TjZsO30LTfZISyBbPiXshAKd.png" }
{ "tmdb_movie_id": 27205, "logo": "none" }
```

A path the title does not offer is rejected. Artwork this service cannot render
is never listed — TMDB serves some logos as SVG, and rasterising vector artwork
from a third party is a larger attack surface than decoding a bitmap.

> Many TMDB posters already carry the title. If the logo duplicates it, pick a
> textless poster from the catalogue — which is what manual selection is for.

| Endpoint | |
|---|---|
| `POST /v1/posters` | Resolve a title, hash, store the specification |
| `GET /v1/posters/{key}.webp` | Render or serve from cache |
| `GET /v1/artwork/{kind}/{id}` | What a title offers, best first |
| `GET /v1/presets` | Preset catalogue with resolved defaults |
| `GET /healthz` `/readyz` | Liveness, readiness |
| `GET /metrics` | Prometheus |

The `POST`/`GET` split exists because a CDN will not cache a `POST`, and above
a 90 % hit rate the CDN is doing most of the work.

The key is `blake3` over the *resolved and clamped* specification plus a
`RENDER_VERSION` constant. Resolution happens at `POST`, so the key covers the
artwork actually used: if it happened at render time, the same URL would
produce different pixels whenever TMDB promoted a different poster, while its
`immutable` header promised otherwise.

**Only rendered results are stored.** Artwork is fetched from TMDB per render
and never written — the service keeps what it produces, not what it consumes.
See [ADR 0007](docs/adr/0007-do-not-persist-source-artwork.md).

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
POSTER_TMDB_API_KEY=… cargo run   # in-memory storage, public TMDB
curl localhost:8080/healthz
```

Or from a container — 7 MB, `scratch`, non-root:

```sh
docker build -f Dockerfile.musl -t poster-service .
docker run -p 8080:8080 -e POSTER_BIND_ADDR=0.0.0.0:8080 poster-service
```

Configuration is `POSTER_`-prefixed environment variables; the full table is in
[PLAN.md § 9](PLAN.md#9-configuration). **`POSTER_TMDB_API_KEY` is required**:
resolving an identifier to artwork is an authenticated call. Either a v3 API
key or a v4 read access token works — the scheme is inferred from the
credential's shape.

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
