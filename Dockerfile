# syntax=docker/dockerfile:1

FROM --platform=$BUILDPLATFORM rust:1.97.1-bookworm AS build

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
 && if [ "$TARGETARCH" = "arm64" ]; then \
      apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu; \
    fi \
 && rm -rf /var/lib/apt/lists/*

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc
ENV CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc

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
