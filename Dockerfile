# syntax=docker/dockerfile:1

FROM --platform=$BUILDPLATFORM golang:1.26.5 AS build

ARG TARGETOS=linux
ARG TARGETARCH=amd64

WORKDIR /src

COPY go.mod go.sum ./
RUN go mod download

COPY cmd ./cmd
COPY internal ./internal

RUN CGO_ENABLED=0 GOOS=$TARGETOS GOARCH=$TARGETARCH \
    go build -trimpath -ldflags="-s -w -buildid=" -o /out/planista ./cmd/planista \
    && mkdir -p /out/data

FROM scratch

LABEL org.opencontainers.image.title="Planista" \
      org.opencontainers.image.description="Publish HTML plans as short public permalinks" \
      org.opencontainers.image.source="https://github.com/Shlok-Bhakta/planista" \
      org.opencontainers.image.licenses="MIT"

COPY --from=build /out/planista /planista
COPY --from=build --chown=65532:65532 /out/data /data

USER 65532:65532
EXPOSE 8080

ENTRYPOINT ["/planista"]
