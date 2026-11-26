# Self-host Macro with Docker Compose

This directory runs Macro (the web app, the core backend services and the
infrastructure they need) from published container images, with no Nix, Rust or
Bun on the host.

```bash
cd docker/selfhost
./setup.sh              # writes .env with generated secrets
docker compose up -d    # pulls the images and starts the stack
```

Open <https://localhost:8090> and sign in with any email address. The one-time
code is delivered to the bundled Mailpit at <https://localhost:8090/mailpit>.
The first start takes several minutes: the database migrates (about 1,000
migrations), OpenSearch and FusionAuth initialise, and the services wait for
each of them.

Macro is licensed under the AGPLv3; see the [FAQ](https://docs.macro.com/faq)
for what self-hosting involves.

## What runs

| Service | Image | Role |
| --- | --- | --- |
| `proxy` | `macro-web` | Caddy. Serves the web app at `/app` and routes every backend path, so the browser sees one origin. |
| `authentication-service`, `connection-gateway`, `contacts-service`, `document-storage-service`, `email-service`, `email-pubsub-workers`, `calendar-service`, `notification-service`, `search-processing-service`, `static-file-service`, `unfurl-service`, `image-proxy-service`, `document-upload-finalizer`, `agent-harness-service` | `macro-services` | The Rust backend. One image holds every binary; each service picks its own with `command:`. The app calls the agent harness (assistant, connectors, scheduled actions) on every page load, so it is not optional. |
| `sync-service` | `macro-sync-service` | Real-time document sync (a Cloudflare Worker, run on workerd). |
| `lexical-service` | `macro-lexical-service` | Document rendering for the sync service and search. |
| `websocket-service` | `macro-websocket-service` | A small WebSocket endpoint the app connects to at `/websocket`. |
| `fusionauth`, `fusionauth-db` | `fusionauth/fusionauth-app` | Identity and passwordless sign-in. |
| `postgres`, `redis`, `kafka`, `search` (OpenSearch) | stock images (`search` is `macro-opensearch`) | Data stores and event bus. |
| `localstack` | `localstack/localstack:4` | Stands in for S3, SQS, DynamoDB and KMS. |
| `mailpit` | `axllent/mailpit` | Catches outgoing mail until you configure SMTP. |
| `migrate`, `create-indices`, `kafka-topics`, `fusionauth-kickstart` | `macro-bootstrap` and stock images | One-shot setup. All are idempotent and run on every `up`. |

This is the service set `just run_local` runs, **minus** the WhatsApp and
Slack-import workers, the AI editing worker, the analytics proxy, OpenBao (the
Passwords vault) and the Gmail, Outlook and model emulators. See
[Limitations](#limitations).

## Requirements

- Docker Engine 24 or later with the Compose plugin (v2.24+).
- On Linux, `vm.max_map_count` of at least 262144 for OpenSearch:
  `sudo sysctl -w vm.max_map_count=262144`.
- `openssl` and `ssh-keygen` on the machine where you run `setup.sh`.

### Resources

Measured on a fresh install on an arm64 machine, with the release images:

| | |
| --- | --- |
| Containers | 26 (about 21 long-running; the rest are one-shot setup) |
| RAM in use | about **3 GB** a few minutes after start, with one user. OpenSearch, LocalStack, FusionAuth, Kafka and Postgres are about 350–650 MB each; the Rust services are 5–280 MB each. Plan for 4 GB free, 8 GB to leave room for a real mailbox |
| Disk, images | about **10 GB** unpacked (about 4 GB to download). The Rust services image is 2.2 GB, OpenSearch 2.2 GB and LocalStack 1.7 GB |
| Disk, data | about 200 MB empty (Postgres, FusionAuth, Kafka) |

Where the weight comes from: the 14 Rust service binaries are 1.3 GB of the
services image, and the platform needs a search engine (OpenSearch: the document
and search services exit without it), a message bus (Kafka), an AWS stand-in
(LocalStack) and an identity provider (FusionAuth), which are JVM or Python
processes. The defaults are already trimmed: small JVM heaps for OpenSearch,
Kafka and FusionAuth, one Kafka partition per topic, Alpine Redis and Postgres
for what does not need more. Raise `OPENSEARCH_JAVA_OPTS` and `KAFKA_HEAP_OPTS`
in `.env` for a large mailbox.

## Configuration

`setup.sh` creates `.env` from [.env.example](.env.example) and fills every empty
secret. It never replaces a value that is already set, so you can run it again
safely. Every setting is documented in `.env.example`.

**Do not change the secrets after the first start.** They are shared by the
services, FusionAuth and the databases; FusionAuth reads its settings only the
first time it boots against an empty database.

### Using your own hostname

```bash
./setup.sh --host macro.example.com --email you@example.com
docker compose up -d
```

Caddy gets a Let's Encrypt certificate for the host, so ports 80 and 443 must
reach this machine. `PUBLIC_ORIGIN` must be the exact origin people type in the
browser: sign-in redirects and presigned file URLs are built from it. If you
change it later, recreate FusionAuth's database (`docker compose down` and remove
the `fusionauth_db_data` volume), or edit the application's redirect URLs in the
FusionAuth admin UI.

For `localhost`, Caddy issues a certificate from its own CA. Trust it in your
browser, or accept the warning:

```bash
docker compose cp proxy:/data/caddy/pki/authorities/local/root.crt ./caddy-root.crt
```

### Video calls

Calls use a LiveKit server, which is optional because it needs UDP ports:

```bash
./setup.sh --calls
docker compose up -d
```

That generates a key pair, turns on the `calls` profile (`COMPOSE_PROFILES=calls`)
and points the backend at `ws://livekit.localhost:7880`. The backend uses that one
URL both to call LiveKit from inside the network and to hand to the browser, so the
host name has to resolve in both places: the `livekit` service answers to it on the
compose network, and browsers resolve `*.localhost` to this machine. Ports 7880/tcp,
7881/tcp and 7882/udp are published on `127.0.0.1`, so only this machine can join.

To let other machines join, set `LIVEKIT_BIND=0.0.0.0`, `LIVEKIT_NODE_IP` to the
public IP of this host, and `LIVEKIT_HOST` and `LIVEKIT_SERVER_URL` to a name they
can resolve, with a TLS terminator in front of port 7880 for `wss://`. That
configuration is untested here. Call recording (LiveKit Egress) and transcription
(a separate agent that uses LiveKit Cloud's speech-to-text) are not included.

### Real email

Set `SMTP_HOST`, `SMTP_PORT`, `SMTP_SECURITY` (`NONE`, `SSL` or `TLS`),
`SMTP_USERNAME`, `SMTP_PASSWORD` and `MAIL_FROM` in `.env`. FusionAuth sends the
sign-in code with the settings it was created with, so apply them **before the
first `up`**, or change them in FusionAuth's admin UI (Tenants → Macro → Email)
afterwards. Remove Mailpit's route from `Caddyfile` if the stack is public.

### FusionAuth admin

The admin UI is bound to `127.0.0.1:9011` and is not part of the public origin.
`setup.sh` prints the admin account it created. That account exists only for the
admin UI; do not sign in to the Macro app with its address, because it is not
registered for the app and the sign-in code cannot be verified.

## Building the images yourself

The compose file builds from source if you ask it to, instead of pulling:

```bash
docker compose build           # all images; the first build takes a long while
docker compose up -d
```

Or build one image with the same tags the compose file expects:

```bash
docker build -f docker/Dockerfile.selfhost -t ghcr.io/macro-inc/macro-services:latest .
# Faster, unoptimised, for a trial:
docker build -f docker/Dockerfile.selfhost --build-arg CARGO_PROFILE=dev ...
```

`MACRO_REGISTRY` and `MACRO_VERSION` in `.env` choose which images the compose
file uses, e.g. a fork's: `MACRO_REGISTRY=ghcr.io/<owner>`.

The images are published by the `Publish self-host images` workflow
(`.github/workflows/publish_selfhost_images.yml`), which builds each image for
`linux/amd64` and `linux/arm64` and pushes `ghcr.io/<owner>/macro-<image>`.

## Operating it

```bash
docker compose ps                    # health of every service
docker compose logs -f email-service # follow one service
docker compose pull && docker compose up -d   # upgrade (migrations run first)
docker compose down                  # stop, keep data
docker compose down -v               # stop and DELETE all data
```

Data lives in named volumes: `postgres_data` (the application database),
`fusionauth_db_data` (accounts), `opensearch_data`, `kafka_data`,
`localstack_data` (uploaded files and queues), `sync_state` (document edits in
flight) and `redis_data`. Back up at least `postgres_data`, `fusionauth_db_data`,
`localstack_data` and `sync_state`, with the stack stopped.

`LOCALSTACK_PERSISTENCE=1` keeps LocalStack's state, including every uploaded
file, in its volume. If the volume is ever lost, `localstack/provision.sh`
recreates the queues, buckets, tables and keys on the next start, but not the
files in them.

## Limitations

- **AI features** (chat, the assistant, agent sessions) call model providers.
  The services start with placeholder keys; set `ANTHROPIC_API_KEY`,
  `OPENAI_API_KEY` or `GOOGLE_GENERATIVE_AI_API_KEY` in `.env` to use them. Agent
  sandboxes for the coder bot (`Code` mode) are not available: they need a Docker
  socket or a Daytona account, and this stack mounts neither. AI document editing
  needs the AI editing worker, which is not included.
- **Connectors** (the catalog of hundreds of apps) are served by Pipedream, a hosted
  third-party broker, so they are off by default (`PIPEDREAM_*` blank). Servers
  added by URL and vendors that run their own MCP server connect directly with
  Macro's own OAuth client, with no broker. Linear, Notion and Jira *imports*
  read the vendor API through the broker and do not work without it yet.
- **Gmail and Outlook** need real OAuth clients. Set `GOOGLE_CLIENT_ID` and
  `GOOGLE_CLIENT_SECRET` **before the first `up`**: they are written into
  FusionAuth's identity provider on its first boot. Until then "Continue with
  Google" is on the sign-in page but fails at Google, and a signed-up account has
  no mailbox (the app logs `NoGmailGrant`; email sign-in itself works). Microsoft
  needs more than a client id and is not wired. The local stack's emulators are
  not included.
- **Billing, GitHub sync and push notifications** are inert: the services read
  placeholder credentials at startup, and the integration does nothing until you
  supply real ones. **Calls** work only with `./setup.sh --calls`; without it the
  document service logs a LiveKit connection error every few seconds.
- Everything AWS-shaped runs on LocalStack. Macro's services have no other
  storage or queue backend, so this is a single-host deployment. It is not
  highly available.
- The services run with `ENVIRONMENT=local`. That is the only mode that targets
  LocalStack, and the one the upstream stack tests.

## Keeping this in sync with the local stack

`just run_local` generates its environment, LocalStack resources, FusionAuth
kickstart and proxy routes from code under `tooling/xtask/crates/xtask_local`.
This directory carries static copies of them:

| Here | Source |
| --- | --- |
| `x-macro-env` in `docker-compose.yml` | `local_env.rs` |
| `localstack/provision.sh` | `resources.rs`, `localstack.rs` |
| `bootstrap/macro-render-kickstart` | `kickstart.rs`, `identity.rs` |
| `Caddyfile` | `proxy.rs`, `inventory.rs` |
| `kafka-topics.txt` | `.github/kafka-cluster-topics.json` |

When you add a service, queue, bucket, table, topic or environment variable
there, add it here too.
