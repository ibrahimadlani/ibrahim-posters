# 5. Content-addressed cache keys over the resolved specification

Date: 2026-08-30

## Status

Accepted

## Context

Rendered posters are served with `Cache-Control: public, max-age=31536000,
immutable`. That header is a promise that the bytes at a URL will never
change. Object storage has no invalidation path and a CDN's purge API is slow
and unreliable at scale, so the promise has to hold structurally rather than
operationally.

Three questions follow. What is hashed, how is it hashed, and what happens
when the renderer changes.

**What.** Hashing the raw request is wrong. Two requests that differ only in
field order, in an override that equals the preset default, or in a value that
clamps to the same result, are the same poster. Hashing the request would give
them distinct keys and render the same image several times — the cache hit
rate target is unreachable if equivalent requests do not converge.

**How.** `serde_json` output is not a safe hash input. The crate makes no
stable guarantee about map key ordering across versions, and float formatting
has changed historically. A silent change in either orphans every key in the
cache at once, which presents as a total cache miss and a cost spike with no
corresponding deploy.

**When the renderer changes.** This is the dangerous case. If the blur
constant changes and the keys do not, the CDN keeps serving posters rendered
by the old code behind a one-year immutable header, and there is no way to
correct it.

## Decision

The key is `blake3(canonical_encoding(ResolvedSpec) || RENDER_VERSION)`,
rendered as 64 lowercase hex characters.

- Hash the **resolved and clamped** specification, after preset merge. Clamping
  runs after merging, never before, so that two requests that clamp to the
  same value converge on one key.
- Encode **field by field in declaration order**, with an explicit length
  prefix on every variable-length field. Serde is not involved. The length
  prefixes are what make the encoding injective: without them, two different
  badge arrays can concatenate to identical bytes.
- Mix in a `RENDER_VERSION` constant. Any change to rendering output requires
  bumping it, which changes every key that renderer produces.

The version bump is enforced by CI, not by discipline: a visual regression
diff without a `RENDER_VERSION` change is a hard build failure.

blake3 over SHA-256 because it is several times faster on short inputs and
the use is content addressing rather than authentication, where its wide
security margin is more than sufficient.

## Consequences

**Positive.** `immutable` is honest. A renderer change invalidates the world
mechanically instead of requiring a purge. Equivalent requests collide onto
one entry, which is what makes a 90 % hit rate reachable. Keys are stable
across deployments, restarts and machines.

**Negative.** Field order in `ResolvedSpec` is load-bearing: reordering the
struct changes every key. This is documented on the type, but it is a
non-obvious hazard for anyone tidying a struct definition. Bumping
`RENDER_VERSION` orphans the entire L2 tier, so a renderer change is followed
by a period of elevated cost until the cache refills — the L1 tier absorbs
part of it, since resized sources survive the bump.

**Neutral.** Orphaned L2 objects are never read again and are removed by a
bucket lifecycle policy rather than by the application.
