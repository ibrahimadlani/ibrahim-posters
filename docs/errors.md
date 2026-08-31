# Error responses

Every failure leaves this service as an RFC 9457 `application/problem+json`
document. One shape, whatever went wrong and wherever — so a caller writes one
error handler rather than one per endpoint.

```json
{
  "type":      "https://github.com/ibrahimadlani/ibrahim-posters/blob/main/docs/errors.md#tmdb_not_found",
  "title":     "Not Found",
  "status":    404,
  "code":      "tmdb_not_found",
  "detail":    "no movie in the TMDB catalogue with id 999999999",
  "hint":      "Check the identifier on themoviedb.org. Films use tmdb_movie_id and series use tmdb_tv_id; the two catalogues number separately, so a valid film id is rarely a valid series id.",
  "retryable": false
}
```

## The fields

| Field | Audience | Answers |
|---|---|---|
| `code` | Your code | Which class of failure is this? |
| `detail` | A human reading a log | What happened *this time*? |
| `hint` | A human deciding what to do | What now? |
| `retryable` | Your retry logic | Could the identical request ever succeed? |
| `type` | A human who does not recognise the code | Where is this documented? |
| `title`, `status` | — | RFC 9457 requires them |

**Branch on `code`, never on `detail`.** The code is a stable contract; the
detail carries specific values and is expected to change.

`detail` and `hint` are separate on purpose. One describes, the other
prescribes. Merged into a single string they produce messages that either
explain without helping or advise without saying what went wrong.

`retryable` is not a guess about load. A `404` is never retryable however long
you wait; a `503` always is. When it is `true` the response also carries
`Retry-After` in seconds.

## Correlation

Every response — success or failure — carries `x-request-id`. An inbound
`x-request-id` is preserved rather than replaced, so an id from a gateway
survives into this service's logs.

Quote it when reporting a problem. It is the only thing that ties a response
you saw to the log line the service wrote.

## Caching

Errors are `no-store`, with one exception: `unknown_key` is cacheable for 60
seconds. An unknown key is unknown permanently, and letting a CDN absorb the
retry storm after a bad link is shared is worth a minute of caching.

---

## Codes

### Requests the caller can fix

#### `malformed_request`
**400.** The body is not valid JSON, or names a field the request type does not
have. Unknown fields are rejected rather than ignored, so a misspelled
`presset` is an error rather than a silently-defaulted `preset`.

#### `invalid_poster_path`
**400.** A `poster` or `logo` value is not a TMDB path. The service never
accepts a URL — only a path matching `/[A-Za-z0-9]{20,60}\.(jpg|png|webp)` —
which is what makes it impossible for a request to reach another host.

#### `unknown_preset`
**400.** No preset by that name. `GET /v1/presets` lists them.

#### `validation_failed`
**422.** The request is well-formed but cannot be satisfied. Covers: naming
neither `tmdb_movie_id` nor `tmdb_tv_id`, or both; too many badges; badge text
that is empty or too long; and artwork the named title does not offer.

The `detail` names the field. For artwork,
`GET /v1/artwork/{kind}/{id}` lists what is actually available.

#### `tmdb_not_found`
**404.** No entry under that identifier. Films and series number separately, so
a valid film id is rarely a valid series id.

#### `unknown_key`
**404.** Nothing was posted under that key. Keys come from `POST /v1/posters`;
an identical specification returns an identical key, so posting again is safe.

The one cacheable error — 60 seconds.

#### `no_artwork_available`
**422.** The title exists but offers no poster this service can render. Not
retryable: TMDB would have to gain suitable artwork first.

Artwork the service cannot render is never offered — TMDB serves some logos as
SVG, and rasterising third-party vector artwork is a materially larger attack
surface than decoding a bitmap.

---

### Failures upstream

#### `source_not_found`
**404.** The artwork this poster was built from is no longer on the TMDB CDN.
Post the request again to resolve current artwork.

404 rather than 502 deliberately: the artwork does not exist, so a retry cannot
succeed, and a gateway status would invite one.

#### `source_too_large`
**502.** The upstream response exceeded the 20 MB cap.

502 rather than 413: the oversized payload is the upstream's, not yours. A 413
would tell you to shrink a request body you never sent.

#### `source_dimensions_exceeded`
**422.** The artwork declares dimensions beyond the decode guard — 8000 px per
side. Read from the file header *before* any decoder allocates, because a
40 KB PNG can declare a 14 GB decode target.

#### `source_decode_failed`
**502.** The artwork was fetched but could not be decoded.

#### `upstream_unavailable`
**502, retryable.** TMDB answered unexpectedly or could not be reached.

#### `upstream_timeout`
**504, retryable.** TMDB did not answer within the configured timeout.

---

### Capacity

#### `overloaded`
**503, retryable.** No render slot became free within the admission wait.

Renders are CPU-bound and bounded to the core count, so this means the service
is genuinely saturated rather than merely busy. Retry after the interval in
`Retry-After`. Sustained rejections mean more replicas or more cores, not a
longer queue — the wait is bounded on purpose.

---

### Faults in the deployment

These are not the caller's fault and the caller cannot fix them. They are
distinct codes so an operator reading a log sees the cause rather than a
generic internal error.

#### `tmdb_credential_missing`
**500.** No `POSTER_TMDB_API_KEY` is configured. Either a v3 API key or a v4
read access token works; the scheme is inferred from the credential's shape.

The service still starts without one — health checks and the preset catalogue
work — so this surfaces on the first request that needs it.

#### `tmdb_unauthorised`
**500.** TMDB rejected the configured credential. Check it has not been revoked
or truncated.

#### `storage_unavailable`
**503, retryable.** Object storage cannot be reached. `GET /readyz` reports the
same condition.

#### `render_failed`
**500.** The service failed to produce the poster. Not a fault in the request;
report it with the `x-request-id`.

---

## Summary

| Code | Status | Retryable | Cacheable |
|---|---|---|---|
| `malformed_request` | 400 | no | no |
| `invalid_poster_path` | 400 | no | no |
| `unknown_preset` | 400 | no | no |
| `validation_failed` | 422 | no | no |
| `no_artwork_available` | 422 | no | no |
| `source_dimensions_exceeded` | 422 | no | no |
| `tmdb_not_found` | 404 | no | no |
| `source_not_found` | 404 | no | no |
| `unknown_key` | 404 | no | **60 s** |
| `source_too_large` | 502 | no | no |
| `source_decode_failed` | 502 | no | no |
| `upstream_unavailable` | 502 | **yes**, 1 s | no |
| `upstream_timeout` | 504 | **yes**, 1 s | no |
| `overloaded` | 503 | **yes**, 1 s | no |
| `storage_unavailable` | 503 | **yes**, 2 s | no |
| `tmdb_credential_missing` | 500 | no | no |
| `tmdb_unauthorised` | 500 | no | no |
| `render_failed` | 500 | no | no |

Pinned by snapshot test, so a change to any row shows up in review.
