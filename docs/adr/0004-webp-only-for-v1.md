# 4. WebP only for v1

Date: 2026-08-30

## Status

Accepted

## Context

Three output formats are plausible: JPEG, WebP and AVIF.

AVIF is the best of the three on compression, typically 20–30 % smaller than
WebP at matched quality, and browser support is now broad enough to rely on.
The problem is encode time. `ravif` at default settings takes on the order of
a second for a 1000x1500 image — more than ten times the entire latency budget
for a render, and an order of magnitude more than libwebp at quality 82.

The budget in PLAN.md § 5 allocates 22 ms to encoding out of a 58 ms total
against an 80 ms p50 target. AVIF does not fit, and no amount of tuning closes
a gap that large; the fast AVIF presets that approach acceptable speed give up
most of the compression advantage that motivated the format.

JPEG encodes fastest but produces files roughly 30 % larger than WebP at
matched quality and cannot represent the alpha channel that the logo and badge
compositing rely on internally.

## Decision

WebP only for v1, at quality 82, method 4.

Method 4 rather than 6: method 6 costs roughly 2.5x the encode time for a
quality difference that does not survive blind comparison at poster
dimensions. Quality 82 rather than 90 for delivery, because the difference is
not visible on the gradient-heavy content posters produce, and the file is
substantially smaller.

The L1 intermediate tier uses quality 90 instead. It will be decoded and
re-encoded, and compounding lossy encodes at delivery quality produces
visible degradation in the gradient regions.

AVIF is explicitly deferred, not rejected. When it is added, it belongs on an
asynchronous pre-warm path: render WebP synchronously, queue AVIF, serve it
through content negotiation once it exists. That design keeps the request path
inside its budget and is the reason the response is content-addressed — an
AVIF variant is a different extension on the same key.

## Consequences

**Positive.** Encoding fits the latency budget with room to spare. One format
means one code path, one set of visual regression references and one cache
entry per key. WebP support is universal in current browsers.

**Negative.** Files are 20–30 % larger than AVIF would be, which is a
bandwidth cost paid on every cache miss at the edge. Clients that would prefer
AVIF cannot get it.

**Neutral.** The `Accept` header is ignored in v1. Adding negotiation later is
additive and does not change any existing URL, because the format is part of
the path rather than of the negotiated representation.
