# Feynman

Feynman is being rebuilt as a Rust CLI-first Agent platform. This workspace contains the first runnable backend slice: tenant-scoped project management, durable conversations and Agent Runs, a Codex App Server adapter, and a terminal chat client.

## Build and test

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Run locally

In one terminal:

```bash
cargo run -p feynman-server
```

The server listens on `127.0.0.1:4317`. It uses `.feynman/rust-platform.sqlite3` for platform records and `.feynman/workspaces/<tenant-id>/<project-id>` for persistent project workspaces. Override these with `FEYNMAN_DB`, `FEYNMAN_DATA_DIR`, and `FEYNMAN_WORKSPACE_ROOT`.

In another terminal:

```bash
cargo run -p feynman -- projects list
cargo run -p feynman -- projects create --name "My project"
cargo run -p feynman -- chat PROJECT_ID
cargo run -p feynman -- external-tools list PROJECT_ID
cargo run -p feynman -- external-tools grant PROJECT_ID dws.aisearch.enterprise
cargo run -p feynman -- external-tools revoke PROJECT_ID dws.aisearch.enterprise
```

Set `FEYNMAN_SERVER_URL` to use another server URL. The server launches `codex app-server` on the first chat turn for a project; Codex must be installed and authenticated in the server process environment. `FEYNMAN_CODEX_BIN` and `FEYNMAN_CODEX_MODEL` can select its executable and model.

The Codex process receives a project-scoped Feynman MCP server through per-process config overrides; Feynman does not edit the user's global Codex configuration. The first external tool is `dws.aisearch.enterprise`, a fixed read-only DWS enterprise search. List tools and grant one to a Project with the commands above; the MCP bridge checks the grant on every call. A missing grant causes the call to be denied, and an owner can grant it before a later Agent turn. The tool is intentionally not an arbitrary DWS command or shell interface.

## Current security boundary

This development server deliberately has no user-login flow yet. Its identity provider exposes only a fixed local Tenant and refuses a non-loopback bind. Do not expose it to a network. Every persistence operation is tenant-scoped, and the CLI cannot select a Tenant by sending a request-body field. Production authentication, user/tenant provisioning, PostgreSQL, centrally managed external connections/credential custody, and remote Agent workspaces are later adapters. In this local adapter, DWS credentials remain in the host's existing DWS profile; they are not copied into the Agent Workspace. Authenticate DWS as the same OS account that runs the Feynman server, or set `FEYNMAN_DWS_BIN` if its executable is not on that process's `PATH`.

Each Codex turn explicitly uses `onRequest` approvals and a `workspaceWrite` sandbox rooted at that Project's persistent workspace. In-sandbox file reads are restricted to that workspace and direct network access is disabled. DWS runs as a Feynman-mediated MCP tool on the server host, outside the Agent Workspace sandbox; its project grant is the authorization gate. This does not grant the Agent general shell or network access. Codex model authentication still comes from the server process's local Codex profile; Feynman does not yet broker provider credentials.

The interactive CLI resumes an active Run, reads its durable ordered event feed, and prompts before answering native Codex approvals. Session liveness Heartbeat and scheduled Agent wake-ups are separate from chat; neither trigger API is enabled yet.

See [`docs/architecture/backend-cli-v1.md`](docs/architecture/backend-cli-v1.md) for routes, boundaries, and the heartbeat distinction. The original architecture source document remains in [`docs/reference`](docs/reference/).
