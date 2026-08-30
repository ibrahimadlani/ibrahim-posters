# Dynamically linked against glibc, on distroless. Kept alongside the musl
# image as the fallback recorded in PLAN.md section 14.2: libwebp is compiled
# from source, and a toolchain change that broke the static build would
# otherwise block a release.
FROM rust:1.98-bookworm AS build
WORKDIR /src

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
# The stub must satisfy every target Cargo.toml declares, benches included:
# cargo refuses to parse a manifest naming a target whose file is absent.
RUN mkdir -p src benches \
    && echo 'fn main() {}' > src/main.rs \
    && echo '' > src/lib.rs \
    && echo 'fn main() {}' > benches/render.rs \
    && cargo build --release \
    && rm -rf src benches

COPY . .
RUN touch src/main.rs src/lib.rs && cargo build --release

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/poster-service /poster-service
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/poster-service"]
