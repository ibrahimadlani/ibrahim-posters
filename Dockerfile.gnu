# Dynamically linked build against glibc. Kept alongside the musl image as the
# fallback described in PLAN.md section 14.2: if vendoring libwebp for musl
# proves unworkable, this is the image that ships.
FROM rust:1.98-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/poster-service /poster-service
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/poster-service"]
