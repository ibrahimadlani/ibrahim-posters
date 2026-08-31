# Architecture decision records

Each record uses the Nygard template: Context, Decision, Consequences. A
record states the situation as it was when the decision was taken and is not
edited afterwards — superseding a decision means adding a new record and
marking the old one `Superseded by NNNN`.

| # | Decision | Status |
|---|---|---|
| [0001](0001-pure-rust-imaging-over-libvips-ffi.md) | Pure Rust imaging over libvips FFI | Accepted |
| [0002](0002-post-then-get-split-over-signed-urls.md) | POST-then-GET split over signed URLs | Accepted (request shape superseded by 0008) |
| [0003](0003-object-storage-over-redis-for-specs.md) | Object storage over Redis | Accepted (L1 tier superseded by 0007) |
| [0004](0004-webp-only-for-v1.md) | WebP only for v1 | Accepted |
| [0005](0005-content-addressed-cache-keys.md) | Content-addressed cache keys | Accepted |
| [0006](0006-trunk-based-branching.md) | Trunk-based branching over git-flow | Accepted |
| [0007](0007-do-not-persist-source-artwork.md) | Do not persist source artwork | Accepted |
| [0008](0008-name-artwork-by-catalogue-identifier.md) | Name artwork by catalogue identifier | Accepted |
