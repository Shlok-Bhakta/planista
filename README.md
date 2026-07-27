# Planista

Planista turns an HTML document into a short public permalink.

```console
$ curl --fail-with-body \
    -H 'Content-Type: text/html; charset=utf-8' \
    --data-binary @plan.html \
    https://plans.example.com/
https://plans.example.com/kr5YQ1Q8c0FQ7tJx
```

It is a single Go binary, a SQLite file, and a `scratch` container. There are no accounts, upload tokens, templates, or JavaScript bundles.

> [!WARNING]
> Planista accepts unauthenticated uploads and serves arbitrary active HTML, including JavaScript. Run it on a dedicated untrusted-content origin with no trusted cookies or other applications. A permalink is unlisted, not private.

## Run with Compose

The included Compose file uses the published image and bind-mounts the database so plans survive container restarts. The repository includes the empty `data` directory so the container runtime does not create it with root ownership:

```console
mkdir -p data
docker compose up -d
```

The container runs as your configured host user so it can write the bind mount. UID and GID default to `1000`; override them when needed:

```console
PLANISTA_UID=$(id -u) PLANISTA_GID=$(id -g) docker compose up -d
```

For an internet-facing deployment, set the exact external origin used in returned permalinks:

```console
PLANISTA_BASE_URL=https://plans.example.com docker compose up -d
```

Terminate TLS and add any desired network-level rate limiting at a reverse proxy. Planista itself speaks HTTP and deliberately does not authenticate uploads.

## API

### Publish

`POST /` with a raw `text/html` body:

```console
curl --fail-with-body \
  -H 'Content-Type: text/html' \
  --data-binary @plan.html \
  http://localhost:8080/
```

A successful upload returns `201 Created`, puts the permalink in the `Location` header, and writes the same URL as a plain-text body. IDs are 16-character unpadded base64url strings containing 96 random bits.

Uploads return:

- `400` for an empty body
- `413` when the HTML exceeds `PLANISTA_MAX_PLAN_BYTES`
- `415` unless the media type is `text/html`
- `507` when `PLANISTA_MAX_PLANS` have been retained

### View

`GET /{id}` returns the stored bytes as active HTML. `HEAD /{id}` returns the same response headers without the body. Plans cannot be listed, edited, or deleted individually.

### Health

`GET /healthz` checks SQLite and returns `ok`.

## Wipe every plan

On startup and every two minutes, Planista generates a new 192-bit wipe URL and prints a ready-to-run command. The previous URL expires immediately.

```console
docker logs --tail 5 planista
```

The output includes a line like:

```text
PLANISTA WIPE (valid until 2026-07-26T20:02:00Z): curl -fsS -X POST 'https://plans.example.com/E0...'
```

Run the current command to delete all plans, truncate the SQLite WAL, and reclaim database space. Invalid or expired wipe URLs return `404`. The wipe token is never stored in SQLite.

## Configuration

| Variable | Default | Description |
| --- | --- | --- |
| `PLANISTA_BASE_URL` | required | Absolute HTTP(S) origin used for returned links |
| `PLANISTA_LISTEN_ADDR` | `:8080` | Server listen address |
| `PLANISTA_DB_PATH` | `/data/planista.db` | SQLite database path |
| `PLANISTA_MAX_PLAN_BYTES` | `1048576` | Maximum bytes in one HTML document |
| `PLANISTA_MAX_PLANS` | `1000` | Maximum retained plans before a wipe |

`PLANISTA_BASE_URL` is required and cannot contain credentials, a path, query, or fragment. Planista never trusts the request `Host` header when constructing links.

## Agent skill

This repository is also a valid Codex skill. [`SKILL.md`](SKILL.md) gives an agent the exact safe publishing workflow and response handling. Install or reference the repository as a skill, set `PLANISTA_URL`, and ask the agent to publish an HTML plan.

## Develop

Planista requires Go 1.26.5.

```console
go test -race ./...
go vet ./...
CGO_ENABLED=0 go build -trimpath -ldflags='-s -w -buildid=' ./cmd/planista
```

The only runtime dependency is SQLite through the pure-Go `modernc.org/sqlite` driver. GitHub Actions tests the service, smoke-tests the container, and publishes `linux/amd64` and `linux/arm64` images to GHCR.

## License

[MIT](LICENSE)
