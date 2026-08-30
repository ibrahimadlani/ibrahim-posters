# 3. Object storage over Redis for specifications and cache tiers

Date: 2026-08-30

## Status

Accepted

## Context

Three things need to persist: resolved specifications keyed by hash, resized
source artwork (L1), and rendered posters (L2). All three are
content-addressed, immutable once written, and read far more often than
written.

Redis is the reflexive choice for a cache and would serve specification
lookups in roughly 1 ms against object storage's 10–30 ms. It is genuinely
faster.

It is also a service. Running it means a deployment to operate, a connection
pool to size, a failure mode to handle, a memory limit to tune against a
working set that grows without bound, an eviction policy to choose, and a
credential to store and rotate. For the image tiers it is a poor fit
regardless: rendered posters are 100–400 KB, and storing them in an in-memory
store means paying RAM prices for data that is read from a CDN edge nearly
every time anyway.

The latency argument deserves scrutiny rather than acceptance. At a steady
state hit rate above 90 %, more than nine in ten requests are served by the
CDN and never reach the service at all. Of the requests that do arrive, the
specification lookup is one step in a path that also includes a render costing
around 58 ms. Trading 20 ms of lookup for an entire additional service is a
bad trade when the render dominates.

## Decision

Use object storage for all three, through the `object_store` crate. No Redis
in v1.

`object_store` is chosen over `aws-sdk-s3` because it abstracts S3, GCS and
Azure behind one trait and ships `LocalFileSystem` and `InMemory` backends.
Those two backends are the reason: integration tests run against `InMemory`
with no containers and no credentials, and local development runs against a
directory. With `aws-sdk-s3` the same capability means hand-writing a trait
and a fake, which is code that exists only to be tested.

## Consequences

**Positive.** One fewer service to deploy, monitor and pay for. One fewer
credential. No eviction policy to tune and no memory ceiling to breach —
storage grows and costs a few cents per gigabyte. Integration tests are fast
and hermetic. Switching cloud providers is a configuration change.

**Negative.** Specification lookup adds roughly 20 ms to the cold path
compared with Redis. Object storage has no native TTL, so the L1 30-day
lifetime is expressed as a bucket lifecycle policy configured outside the
application, which is a piece of required infrastructure that is not visible
in the code.

**Neutral.** If specification lookup ever shows up in a p99 profile, a
bounded in-process `moka` cache in front of object storage recovers most of
the difference without adding a service. That is a smaller step than adopting
Redis, and it should be taken first.
