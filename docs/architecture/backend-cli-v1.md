# Rust Backend and CLI — V1 Implementation Boundary

This document records the first runnable Rust slice. It includes the provider-neutral external-tool seam and a local DWS read-only search adapter. Production authentication, centrally managed external-source connections/credentials, and remote sandbox orchestration remain explicit follow-up work.

## Runtime path

```text
feynman CLI
  └─ HTTP/JSON v1 ──> Feynman Server
                       ├─ IdentityProvider → Principal (user + tenant; never request-body tenant)
                       ├─ PlatformService → project / conversation / run use cases
                       ├─ PlatformRepository → SQLite, every query scoped by tenant_id
                       ├─ WorkspaceProvider → persistent local workspace by tenant/project
                       └─ AgentRuntime → Codex App Server process
                                            ├─ normalized events → durable run event log
                                            └─ project MCP (per Codex process)
                                                 └─ ExternalToolService (Tool Grant check)
                                                      └─ DWS adapter → host `dws` profile
```

The CLI submits a message, receives an `AgentRun`, then reads its ordered event feed using `after=<sequence>`. If the terminal disconnects, it can resume from the last sequence. The feed is persisted; it does not depend on a live SSE connection.

## Domain ownership

- A `Project` belongs to one Tenant and has one long-lived Main Agent.
- An `AgentSession` is the durable association between that Main Agent, its Project, and its persistent Workspace. It may outlive a particular Codex process.
- A `Conversation` is the human-readable thread; its Codex thread ID is runtime metadata.
- An `AgentRun` is one invocation caused by a user message or later event. It references the Agent Session and owns a monotonic event sequence.
- A `Tool Invocation` is authorized against the current Project on every call. In this slice, the Agent can request the fixed DWS enterprise search through MCP; an owner manages the durable project `Tool Grant` with the Feynman CLI. A `Tool Result` contains structured source/time metadata but is not yet persisted as an `Observation`.
- `Session Heartbeat` liveness and a periodic wake-up are separate: liveness updates the Session; a scheduled wake-up may create a new Agent Run. Neither is implemented as an external endpoint yet.

## HTTP contract

| Route | Purpose |
| --- | --- |
| `GET /v1/health` | Local server health |
| `GET /v1/projects` | List projects in the authenticated Tenant |
| `POST /v1/projects` | Create a Project; body contains `name`, not `tenant_id` |
| `GET /v1/projects/{project_id}` | Read one tenant-scoped Project |
| `GET /v1/projects/{project_id}/external-tools` | List known external tools and this Project's grants |
| `PUT /v1/projects/{project_id}/external-tools/{tool_id}` | Grant a known external tool to this Project |
| `DELETE /v1/projects/{project_id}/external-tools/{tool_id}` | Revoke a Project's tool grant |
| `GET /v1/projects/{project_id}/conversation` | Read conversation, messages, runs |
| `POST /v1/projects/{project_id}/messages` | Create a message and start an Agent Run |
| `GET /v1/projects/{project_id}/runs/{run_id}/events?after=N` | Read run status and events after cursor N |
| `POST /v1/projects/{project_id}/runs/{run_id}/approvals/{request_id}` | Answer a pending native Codex approval |

## CLI contract

```text
feynman --server URL health
feynman --server URL projects list
feynman --server URL projects create --name "Project name"
feynman --server URL projects show PROJECT_ID
feynman --server URL external-tools list PROJECT_ID
feynman --server URL external-tools grant PROJECT_ID dws.aisearch.enterprise
feynman --server URL external-tools revoke PROJECT_ID dws.aisearch.enterprise
feynman --server URL conversation PROJECT_ID
feynman --server URL chat PROJECT_ID
```

`FEYNMAN_SERVER_URL` may replace the repeated `--server` option. The interactive chat prints streamed deltas and tool activity, and asks before answering a native approval request.

## Security and deployment boundary

The current `DevIdentityProvider` supplies only the local development Principal (`local-user` in Tenant `local`). The server refuses any non-loopback bind until a real identity provider is configured. It does not accept a tenant ID from CLI flags or request bodies. Therefore this is a tenant-isolated local development foundation, not yet a remotely usable multi-tenant service.

Each Codex turn requests native `onRequest` approvals, uses `workspaceWrite` with readable/writable roots restricted to that Project's canonical Workspace path, and disables in-sandbox network access. The server injects the Feynman MCP bridge into only that Codex process with config overrides; it does not modify global Codex configuration. The bridge exposes only fixed, typed external-tool operations and checks the Project's `Tool Grant` for every invocation. An ungranted call is denied with CLI guidance; after an owner grants access, the Agent can call it in a later turn.

The first adapter, `dws.aisearch.enterprise`, invokes only `dws aisearch enterprise ... --format json` with allowlisted arguments. It is a read-only discovery search; it does not access or mutate the matched resource. The DWS CLI runs on the Feynman server host and uses that OS account's existing DWS profile. Credentials are not copied into the Agent Workspace. This local profile is a development shortcut, not the planned multi-tenant External Connection vault. DWS Tool Results include source and observation time but are not yet stored in the durable Evidence model. There is no per-call human approval prompt: the explicit, revocable project grant is the gate for this low-risk read operation.

Codex model authentication is still supplied by the local Codex profile available to the Server process; provider credential brokering is not implemented yet.

SQLite and the filesystem workspace provider are V1 local adapters. The `PlatformRepository`, `IdentityProvider`, and `WorkspaceProvider` interfaces are the replacement seams for PostgreSQL, user authentication, and a persistent remote sandbox such as GKE Agent Sandbox. The workspace path is derived on the server from Tenant and Project IDs; clients cannot choose arbitrary server paths.

The Codex adapter speaks the App Server JSONL protocol. OpenAI describes App Server as the integration for product-owned persistent conversations, streamed events, tools, and approvals ([Codex App Server overview](https://developers.openai.com/codex/app-server)). The adapter is isolated behind `AgentRuntime`; no Codex-specific IDs appear as the platform's durable identity.
