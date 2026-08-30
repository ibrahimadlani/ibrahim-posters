# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries below are generated from commit subjects by
[git-cliff](https://git-cliff.org); regenerate with `git cliff -o CHANGELOG.md`.


## [0.2.0] - 2026-08-30

### Added

- Add the http surface for poster creation and retrieval (#11)
- Bound concurrent renders and coalesce duplicate work (#12)



## [0.1.0] - 2026-08-30

### Added

- Resolve poster requests into content-addressed specifications (#6)
- Fetch tmdb artwork under byte and dimension guards (#8)
- Add object storage for cache tiers and specifications (#9)
- Render posters from resolved specifications (#10)

### Testing

- Cover the ssrf guards and resolved specification geometry (#7)



## [0.0.1] - 2026-08-30

### Documentation

- Add v1 implementation plan (#2)
- Record architecture decisions for v1 (#3)


[unreleased]: https://github.com/ibrahimadlani/ibrahim-posters/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/ibrahimadlani/ibrahim-posters/compare/v0.2.0...v1.0.0
[0.2.0]: https://github.com/ibrahimadlani/ibrahim-posters/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ibrahimadlani/ibrahim-posters/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/ibrahimadlani/ibrahim-posters/releases/tag/v0.0.1
