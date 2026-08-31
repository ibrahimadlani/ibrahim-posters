# 8. Name artwork by catalogue identifier

Date: 2026-08-31

## Status

Accepted. Supersedes the request shape in
[0002](0002-post-then-get-split-over-signed-urls.md), which remains accurate
about the endpoint split.

## Context

Until v2 a caller supplied artwork paths directly:

```json
{ "source": "/kqjL17yufvn9OVLyXYpvtyrFfak.jpg",
  "logo":   "/aaaaaaaaaaaaaaaaaaaaaaaaaaaa.png" }
```

That required the caller to have already talked to TMDB, which every caller of
a movie-poster service has to do anyway — so the service was pushing a lookup
onto each of its clients rather than doing it once itself. It also meant the
service could not help: it had no idea which of a title's seventy-six posters
was the good one, because it never saw the list.

Two further problems followed from taking paths on trust. A caller could
composite one film's logo onto another film's poster with no indication that
anything was wrong, and a caller had no way to discover what artwork existed
short of querying TMDB themselves.

## Decision

Callers name a **catalogue entry**, not a path:

```json
{ "tmdb_movie_id": 27205 }
{ "tmdb_tv_id": 1396 }
```

The service resolves the identifier through the TMDB API, ranks what it finds,
and composites the best. A caller who wants something else browses
`GET /v1/artwork/{kind}/{id}` and names a path from it:

```json
{ "tmdb_movie_id": 27205, "logo": "/eS5TjZsO30LTfZISyBbPiXshAKd.png" }
{ "tmdb_movie_id": 27205, "logo": "none" }
```

An explicit choice is checked against the title's own catalogue.

**Resolution happens at `POST`, not at `GET`.** This is the load-bearing part.
A rendered poster is served with a one-year `immutable` directive, and its key
is a hash of the resolved specification. If the identifier were resolved during
a render, the same key would produce different artwork whenever TMDB promoted a
different poster — the response would change while the URL promised it could
not. Putting the resolved *paths* in the specification makes the key cover the
artwork actually used, and leaves `GET` free of metadata calls, which matters
because `GET` is the cached path and `POST` is not.

The ranking is: requested language, then language-neutral, then any other;
within a band, highest rated, votes breaking a rating tie. The editorially
primary poster is promoted to the front regardless, because TMDB's designated
poster is a better default than the highest-voted one — a poster accumulates
votes for being striking, which is not the same as being the poster of record.

Artwork this service cannot render is never offered. TMDB serves some logos as
SVG, and rasterising vector artwork fetched from a third party through `resvg`
is a materially larger attack surface than decoding a bitmap. This is not
hypothetical: Breaking Bad's second-highest-voted logo is an SVG, so a service
that offered everything would select it routinely.

## Consequences

**Positive.** A caller needs only an identifier, which is the thing they
already have. The default is a reasoned choice rather than whatever the caller
happened to paste. The catalogue endpoint makes that default visible — its
first entry is exactly what `auto` selects — so a caller can see what they are
getting and change it. Mixing artwork between titles is rejected rather than
silently rendered.

**Negative.** A breaking change: every v1 client stops working, and
`POSTER_TMDB_API_KEY` becomes required where no credential was needed before.
`POST` now costs two TMDB API calls, taking it from roughly 1 ms to roughly
150 ms — acceptable because `POST` sits outside the cache by design, but it is
no longer trivially cheap. The service contacts a second host, though under the
same constraint as the first: a configured base with an integer interpolated
into the path.

**Neutral.** Callers who genuinely want arbitrary artwork are no longer served.
That is a deliberate narrowing: this is a service for compositing posters of
titles TMDB knows about, and the previous shape let it be used as a general
image compositor by accident.
