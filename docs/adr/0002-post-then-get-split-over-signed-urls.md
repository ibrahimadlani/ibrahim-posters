# 2. POST-then-GET split over signed URLs

Date: 2026-08-30

## Status

Accepted

## Context

Posters are described by a specification too large and too structured to fit
comfortably in a query string: a source path, a preset, a badge array and a
set of numeric overrides. The API needs to accept that specification and
return an image, and the image needs to be cacheable by a CDN.

A CDN will not cache a `POST`. That single fact drives the design, because at
a target hit rate above 90 % the CDN is doing most of the work — an
uncacheable API is one that renders every request.

Three options were considered.

1. **`POST` returning the image directly.** Simplest to use, uncacheable at
   the edge. Every request is a render. Rejected on cost.
2. **`GET` with the whole specification in the query string.** Cacheable, but
   the URL grows past practical limits with a badge array, and equivalent
   specifications written with different parameter ordering produce distinct
   URLs and therefore distinct cache entries. Cache fragmentation defeats the
   purpose.
3. **Signed URLs.** A `GET` carrying the specification plus an HMAC. Cacheable
   and tamper-proof, but it inherits the length and ordering problems of
   option 2, and adds a signing key: a secret to distribute, rotate and leak.
   Rotation is particularly awkward here, because rotating the key changes
   every URL and therefore invalidates the entire CDN cache.

## Decision

Split the operation across two endpoints.

`POST /v1/posters` validates the request, resolves it against its preset,
clamps it, hashes the result, writes the specification to object storage and
returns the key with a canonical URL. It is cheap: validation and a hash, plus
one small write. No image work happens.

`GET /v1/posters/{key}.webp` looks the specification up by key, renders or
serves from cache, and responds with
`Cache-Control: public, max-age=31536000, immutable`.

## Consequences

**Positive.** The `GET` is trivially cacheable, and its URL is short and
opaque. The key is a hash of the resolved specification, so equivalent
requests converge on one cache entry regardless of how they were written — the
property option 2 could not provide. No signing key exists, so none can leak
or need rotation. The specification stored alongside the key makes any
rendered poster reproducible and debuggable after the fact.

**Negative.** Clients make two calls instead of one. A `GET` for a key that
was never `POST`ed returns 404, so clients cannot construct URLs
independently. Specifications accumulate in storage indefinitely; they are
small JSON documents, so the cost is negligible, but it is unbounded and will
eventually want a lifecycle policy.

**Neutral.** The `POST` is not idempotent in the HTTP sense but is idempotent
in effect: posting the same specification twice returns the same key and
overwrites an identical object.
