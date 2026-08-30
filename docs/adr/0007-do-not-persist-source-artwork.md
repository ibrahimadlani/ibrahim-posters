# 7. Do not persist source artwork

Date: 2026-08-30

## Status

Accepted. Supersedes the L1 tier described in
[0003](0003-object-storage-over-redis-for-specs.md), which remains accurate
about the choice of storage substrate.

## Context

The original design had two cache tiers. L2 held rendered posters. L1 held
TMDB artwork — the background resized to the output frame, and the title logo
in its original encoding — so that a second poster built from the same source
skipped both the upstream fetch and the resize.

That is a real saving. The resize is the second most expensive stage in the
pipeline at 12.4 ms measured, and the fetch adds 30–80 ms of round trip. On a
title with several poster variants, L1 turned four fetches and four resizes
into one.

It also meant the service kept copies of images it did not create. TMDB
artwork belongs to the studios that produced it and is served by TMDB under
terms that permit display, not redistribution. A bucket holding thousands of
resized posters is a redistribution of that artwork, however incidental the
intent — and it makes the service's storage a thing that has to be reasoned
about legally rather than only operationally.

The question is not whether L1 is faster. It plainly is. The question is
whether the service should hold that data at all.

## Decision

Store only what the service produces: rendered posters and the specifications
that describe them. Fetch background artwork and logos from TMDB on every
render.

The storage layout drops to two prefixes:

```
l2/{key}.webp     rendered posters
spec/{key}.json   resolved specifications
```

Nothing fetched from upstream is written anywhere. The bytes exist in memory
for the duration of one render and are dropped with it.

## Consequences

**Positive.** The service stores nothing it did not create, so the copyright
question does not arise: a rendered composite is the service's own output in a
way that a resized copy of someone's poster is not. Storage shrinks to the
rendered results and their specifications, both small and both
content-addressed. One fewer tier to reason about, one fewer lifecycle rule to
configure, and one fewer way for a cache to hold something stale.

**Negative.** Every render pays a fetch and a resize — roughly 25 ms combined
that L1 previously amortised. Concurrent renders of *different* posters built
from one source each fetch that source independently; single-flight still
collapses duplicate work on the same poster, but it cannot collapse work that
merely shares an input. TMDB sees more traffic from this service than it
otherwise would.

The cost is bounded by how often a render happens at all. Rendered posters are
cached in L2 and served from a CDN under a one-year immutable directive, so at
the target hit rate a render — and therefore a fetch — is a small fraction of
requests. The saving L1 offered applied only to that fraction.

**Neutral.** Nothing about this is irreversible. If upstream traffic ever
becomes the constraint, the options in increasing order of commitment are a
bounded in-process cache with a short lifetime, which holds nothing across a
restart and is not storage in any meaningful sense; and then, if that is not
enough, reinstating L1 with the licensing question answered rather than
avoided.
