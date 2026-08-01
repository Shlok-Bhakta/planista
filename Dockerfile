# syntax=docker/dockerfile:1

FROM rust:1.97.1-bookworm AS build

ARG TARGETARCH
WORKDIR /src

RUN apt-get update \
 && apt-get install -y --no-install-recommends musl-tools \
 && case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl > /target.txt ;; \
      arm64) echo aarch64-unknown-linux-musl > /target.txt ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && rustup target add "$(cat /target.txt)" \
 && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src

RUN cargo build --locked --release --target "$(cat /target.txt)" \
 && cp "target/$(cat /target.txt)/release/planista" /out-planista \
 && mkdir -p /out/data

FROM scratch

LABEL org.opencontainers.image.title="Planista" \
      org.opencontainers.image.description="Publish files as short public permalinks" \
      org.opencontainers.image.source="https://github.com/Shlok-Bhakta/planista" \
      org.opencontainers.image.licenses="MIT"

COPY --from=build /out-planista /planista
COPY --from=build --chown=65532:65532 /out/data /data

USER 65532:65532
EXPOSE 8080

ENTRYPOINT ["/planista"]
