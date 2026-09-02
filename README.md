# Poster Service

HTTP API that composites custom movie posters from TMDB artwork: a background
image, a gradient gaussian blur rising from the bottom edge, a tinted ramp for
legibility, a title logo, an optional genre and rating line, and a single
badge coloured from the poster it sits on.

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
    "badges": [{ "text": "#6 Today" }],
    "caption": { "genre": "Action", "rating": 6.5 }
  }'
# { "key": "0d31a4e0…", "url": "https://…/v1/posters/0d31a4e0….webp" }

curl -o poster.webp localhost:8080/v1/posters/0d31a4e0….webp
```

Series work the same way with `"tmdb_tv_id"`.

> A [Postman collection](postman/) covering every endpoint is in `postman/`,
> with requests chained so browsing artwork feeds straight into creating a
> poster.

### Choosing the artwork

Posters and logos are chosen by opposite rules, because they play opposite
roles.

A **poster** is the background, so the service offers *only* artwork with no
language — on TMDB those are the textless versions, without the title
treatment or the credits block. A poster carrying its own title is not a worse
background, it is the wrong kind of thing.

A title with no textless poster falls back to everything it has. That is not
hedging: measured across twenty titles, every popular one offered between 4
and 32 textless posters, but four of ten obscure ones offered none — without
the fallback they could not be rendered at all. You can tell which happened
from the response: if no option's `language` is `null`, the fallback ran.

A **logo** *is* the title, so the requested language comes first, then
language-neutral, then anything else.

Within any band: highest rated, with votes breaking a tie.

Offering titled posters the way logos are offered gives a poster with its title
printed twice — once in the artwork and once in the logo over it — which is
what the filter exists to avoid.

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

## The badge, the caption, and one accolade per poster

A poster carries **at most one badge**. That is a policy, not a capacity
limit: the badge's job is to give the eye a single thing to land on above the
artwork, and two of them say nothing because neither reads as the point. Pick
the one that matters — an IMDb rank, an award nomination, a trending position.
A request with more is rejected.

Under the logo sits an optional caption, `"caption": { "genre": …, "rating": …
}`, rendered as `Action · ★ 6.5`. Both halves are optional and either alone is
a valid caption; the logo is lifted to clear it automatically. Omit the field
entirely and the poster renders without it.

### Colours are derived from the artwork

Under the `standard` preset the badge is not a fixed colour. Its fill is the
**dominant colour of the poster's top region** — the mode of a quantised
histogram, with near-black and near-white excluded so that a vignette does not
win — and its text is whichever of black and white has the higher WCAG
contrast against that fill. A yellow poster gets a yellow badge with black
text; a dark one gets a dark badge with white text, from the same rule.

Two more treatments make that legible whatever the artwork does:

- an **inset shadow** under the top edge, at 85% opacity on the first row and
  released linearly to nothing by a quarter of the way down, so the badge sits
  on a predictable ground;
- a **tinted band** at the bottom that blends toward a dark warm neutral
  rather than toward black, so a wall of posters shares one footing while the
  blur keeps each one's own shapes visible through it.

## Presets

`standard`, `cinematic`, `minimal`, `poster_wall`. Each sets the blur band
height and sigma, the darkening strength and colour, the top shadow, logo
geometry, badge shape and caption style. `standard` is the only one that
derives its badge colour from the artwork; the others keep fixed palettes and
text-sized pills. The continuous values can be overridden per request and are
clamped after the merge.

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

## Playground

`site/` is a single page that exercises every endpoint: browse a title's
artwork as thumbnails, pick a poster and logo, choose a preset and badges,
render, and read the caching headers back. No build step.

```sh
POSTER_TMDB_API_KEY=… cargo run --release   # the service, on :8080
python3 -m http.server 4173 -d site         # the page, on :4173
```

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
| [docs/errors.md](docs/errors.md) | Every error code, what it means, what to do |
| [docs/deployment.md](docs/deployment.md) | Operating the service |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Conventions |
| [CHANGELOG.md](CHANGELOG.md) | Release history |
| [site/](site/) | Interactive playground — browse artwork, pick, render |
| [postman/](postman/) | Importable Postman collection and environment |

## Licence

[MIT](LICENSE). Bundled fonts are [Inter](https://rsms.me/inter/) under
OFL-1.1; see `assets/fonts/Inter-LICENSE.txt`.
