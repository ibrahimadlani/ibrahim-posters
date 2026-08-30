# 1. Pure Rust imaging over libvips FFI

Date: 2026-08-30

## Status

Accepted

## Context

The service composites posters: decode, resize, blur, gradient, alpha
compositing, text rasterisation and encode. Two families of implementation are
available.

**libvips via FFI.** Mature, demand-driven, streaming, and faster than
anything in the Rust ecosystem for large pipelines. It is what most production
image services are built on, and for good reason. The cost is a C library
dependency: a system package or a vendored build in every image, `unsafe` at
the boundary, and a memory-safety surface that our `#![forbid(unsafe_code)]`
posture would have to make an exception for.

**Pure Rust.** `zune-jpeg` for decode, `fast_image_resize` for SIMD resampling,
`tiny-skia` for compositing, `resvg` for text. Each crate is narrower than
libvips and collectively they are slower on very large images, but they are
`unsafe`-free at our call sites and require no system libraries.

The deciding factor is the size of the images. Posters are 1000x1500, and at
w2000 they are 2000x3000. This is small. libvips' architectural advantage is
demand-driven streaming over images too large to hold in memory — an advantage
that does not apply when the entire working set is 12 MB and fits in L3 on a
modern server part.

## Decision

Use pure Rust crates for the entire pipeline.

Accept one exception: WebP encoding binds to libwebp through the `webp` crate.
No pure-Rust lossy WebP encoder exists — `image-webp` encodes lossless only —
and lossy encoding at quality 82 is not optional for the size budget. The
exception is narrow, confined to one module, and the binding surface is a
single call rather than a pipeline.

## Consequences

**Positive.** `#![forbid(unsafe_code)]` holds across the crate. Builds need no
system packages beyond libwebp. The renderer is a pure function over byte
buffers, which is what makes pixel-level regression comparison between
versions possible at all. Cross-compilation to musl stays tractable.

**Negative.** We give up libvips' performance ceiling. If poster dimensions
ever grow by an order of magnitude, this decision should be revisited — the
reasoning above is explicitly contingent on the working set fitting in cache.
We also depend on a set of smaller crates with smaller maintainer pools than
libvips has.

**Neutral.** The libwebp exception means the musl release build must vendor
and statically link libwebp. This is verified in CI from the first milestone
rather than discovered at release time.
