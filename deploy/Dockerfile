# Multi-stage build producing a small image with a single static-ish binary.
# Build context is the repo root:  docker build -f deploy/Dockerfile -t homelab-hub .

FROM rust:1-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release
# Created here so it can be copied in with nonroot ownership below; the final
# image has no shell to mkdir/chown.
RUN mkdir /data

# Distroless "cc" provides glibc + libgcc, which the default build needs.
# (TLS is rustls and SQLite is bundled, so no OpenSSL/system sqlite required.)
FROM gcr.io/distroless/cc-debian12
COPY --from=build /app/target/release/homelab-hub /usr/local/bin/homelab-hub
COPY --from=build --chown=nonroot:nonroot /data /data

# Mount your config at /config/config.json and persist history in /data.
VOLUME ["/data"]
EXPOSE 8080
ENV HUB_CONFIG=/config/config.json \
    HUB_DB=/data/hub.db \
    HUB_BIND=0.0.0.0:8080 \
    RUST_LOG=info

USER nonroot
ENTRYPOINT ["/usr/local/bin/homelab-hub"]
