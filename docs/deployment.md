# Deployment

## Images

Two are published to GHCR on every `v*` tag:

| Tag | Base | Use |
|---|---|---|
| `ghcr.io/ibrahimadlani/ibrahim-posters:vX.Y.Z-musl` | `scratch` | Default |
| `ghcr.io/ibrahimadlani/ibrahim-posters:vX.Y.Z-gnu` | distroless `cc` | Fallback |

The musl image is statically linked and runs on `scratch`: the binary, a CA
certificate bundle, and nothing else. No shell, no package manager, and no CVE
surface from a base distribution the service never calls into.

The glibc image exists because `libwebp` is compiled from source by
`libwebp-sys`, and a toolchain change that broke the static build would
otherwise block a release. It is the recorded fallback from `PLAN.md` § 14.2,
not a variant anyone is expected to choose.

Both run as a non-root user.

### The `local` backend needs a mounted volume

The musl image is `scratch` and runs as uid 65534. There is no `/tmp`, and the
container root is not writable by that user, so `POSTER_STORAGE_BACKEND=local`
fails at startup unless the path is a mounted volume:

```
Error: could not open storage at /tmp/store: Permission denied (os error 13)
```

```sh
docker run -v /host/path:/data \
  -e POSTER_STORAGE_BACKEND=local \
  -e POSTER_STORAGE_LOCAL_PATH=/data \
  ghcr.io/ibrahimadlani/ibrahim-posters:latest-musl
```

`memory` and `s3` need no filesystem and work as-is. This is a property of the
image being minimal rather than a defect: a writable root would be a larger
attack surface than the local backend is worth.

## Configuration

Every service setting is a `POSTER_`-prefixed environment variable. The full
table is in [`PLAN.md` § 9](../PLAN.md#9-configuration). The minimum for a
real deployment:

```sh
POSTER_TMDB_API_KEY=…                       # required
POSTER_STORAGE_BACKEND=s3
POSTER_STORAGE_BUCKET=my-poster-cache
POSTER_PUBLIC_BASE_URL=https://posters.example.com
AWS_REGION=eu-west-1
```

`POSTER_TMDB_API_KEY` accepts either a v3 API key or a v4 read access token;
the scheme is inferred from the credential's shape. A missing credential does
not stop the process starting — health checks and the preset catalogue still
work — but every poster request fails with `tmdb_credential_missing`, which
names the variable to set.

AWS credentials use the conventional unprefixed names and are read by
`object_store`, which also resolves IAM roles and instance metadata — so on
EKS or ECS with a task role, no credential variables are needed at all.

A misspelled `POSTER_` variable fails at startup rather than being silently
ignored. Variables outside that namespace are not read.

## Storage

One bucket holds two prefixes:

```
l2/{key}.webp     rendered posters
spec/{key}.json   resolved specifications
```

**No lifecycle rule is needed, and none should be set.** Rendered posters are
served with a one-year `immutable` directive, and a specification is what
makes its poster reproducible after the fact. Both are small and
content-addressed, so nothing here goes stale.

**Source artwork is never written.** Backgrounds and logos are fetched from
TMDB on each render and dropped with it. The bucket contains only output this
service produced, which is what keeps its contents a question about cost
rather than about redistribution rights. See
[ADR 0007](adr/0007-do-not-persist-source-artwork.md).

The practical consequence for capacity planning is that every render costs one
or two TMDB fetches. Above the target hit rate a render is a small fraction of
requests, but a cold start — a new deployment, or a `RENDER_VERSION` bump that
orphans L2 — produces a burst of upstream traffic proportional to how quickly
the cache refills.

## CDN

Point a CDN at `GET /v1/posters/{key}.webp` and let it cache on the response
headers. The service sets `public, max-age=31536000, immutable`, which is
honest because the key is a hash of the resolved specification *including*
`RENDER_VERSION` — a renderer change cannot be served from a stale entry
because it cannot produce the same key.

`POST /v1/posters` must not be cached. It is cheap by design so that it can
sit outside the cache: validation, a hash, and one small write.

Error responses are `no-store`, except `unknown_key`, which is cacheable for
60 seconds so the CDN absorbs the retry storm that follows a bad link being
shared.

## Health and readiness

| Path | Meaning |
|---|---|
| `GET /healthz` | The process can answer. Checks nothing else. |
| `GET /readyz` | Object storage is reachable. |

Wire liveness to `/healthz` and readiness to `/readyz`, not both to the same
endpoint. A liveness probe that consults a dependency turns that dependency's
outage into a restart loop, removing capacity at exactly the moment the system
is already degraded.

Note that with the `local` backend a deleted prefix directory lists as empty
and reports *ready*; the S3 backend returns an error. The local backend's
readiness signal is weaker, which matters for development only.

## Capacity

Renders are CPU-bound. `POSTER_RENDER_CONCURRENCY` defaults to the core count
the process observes, which is the right number: admitting more work than
there are cores raises latency without raising throughput.

**Set it explicitly when the container's CPU quota is lower than the host's
core count.** A process in a 2-CPU cgroup on a 64-core node will otherwise
admit 64 concurrent renders and thrash.

A request that cannot get a render slot within 50 ms is rejected with `503`
and `Retry-After: 1`. Sustained rejections mean more replicas or more cores,
not a longer queue.

## Metrics

`GET /metrics` serves Prometheus text. Ten series; the ones worth alerting on:

| Metric | Signal |
|---|---|
| `poster_admission_rejected_total` | Rising means capacity is the constraint |
| `poster_cache_lookups_total{tier="l2",result="miss"}` | Rising miss ratio means the cache is not doing its job, and each miss is an upstream fetch |
| `poster_upstream_duration_seconds{asset="background"}` | TMDB CDN latency, on the critical path of every render |
| `poster_metadata_duration_seconds` | TMDB API latency, on the critical path of every `POST` |
| `poster_request_duration_seconds` | Latency, by route |
| `poster_render_slots_available` | Sitting at zero means saturation |

Latency histogram buckets bracket both design targets — 80 ms p50 and 250 ms
p99 — so neither is read off the edge of a histogram.

`x-cache` on a poster response distinguishes three cases: `HIT` from L2,
`MISS` rendered for this request, and `COALESCED` served after waiting for
another request to render the same key. A rising `COALESCED` rate is
concurrent demand for cold keys, which is a capacity signal rather than a
cache one.

## Deploying a renderer change

Any change that alters rendered pixels **must** bump `RENDER_VERSION` in
`src/lib.rs`. CI enforces it: the visual regression references live under
`tests/visual/reference/v{RENDER_VERSION}/`, so a rendering change without a
bump fails against the current references, and a bump without regenerated
references fails on a missing directory.

Bumping orphans the entire L2 tier. The service will render cold for a period
proportional to traffic while it refills; the L1 tier survives, so the
upstream fetch and the resize are not repeated. Orphaned L2 objects are never
read again and are removed by the bucket lifecycle policy.

## Rolling back

Roll back to a previous image tag. No cache invalidation is needed: an older
renderer has an older `RENDER_VERSION` and therefore its own keys, so it reads
and writes entries the newer one never touches. Rolling forward again finds
its own entries still present.
