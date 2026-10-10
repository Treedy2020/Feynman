# Rust Backend and CLI — V1 Implementation Boundary

This document records the original Rust slice. The current
[local-first Main Agent architecture](local-first-main-agent-v1.md) adds Skill/CLI
delegation, Project Tasks/Schedules, and OpenSandbox native Workspace orchestration.
The local-only runtime details below describe that original adapter.

The subsequent [DWS device authorization slice](dws-device-authorization-v1.md)
adds project-scoped asynchronous sign-in, public links/status, and a local private
credential store. It does not implement provider revocation, sandbox credential
delivery, or automatic business-tool grants.

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
                                                      └─ External Access → explicitly bound CLI account
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

The current extension exposes `GET /v1/status`, `GET /v1/projects/{id}/status`,
`GET/POST /v1/projects/{id}/workspace`, `POST .../workspace/actions`,
`GET/POST .../tasks`, `GET/DELETE .../tasks/{task_id}`, `GET/POST .../schedules`
and `PUT .../schedules/{schedule_id}`. Task requests contain `title`/`prompt`;
Schedules add `interval_seconds`, RFC3339 `at`, and `enabled` (false by default).
Workspace creation takes only `provider`; private connection/image configuration
belongs to the operator, not the caller. Actions are refresh/stop/resume/restore/renew.

| Route | Purpose |
| --- | --- |
| `GET /v1/health` | Local server health |
| `GET /v1/projects` | List projects in the authenticated Tenant |
| `POST /v1/projects` | Create a Project; body contains `name`, not `tenant_id` |
| `GET /v1/projects/{project_id}` | Read one tenant-scoped Project |
| `GET /v1/projects/{project_id}/external-tools` | List known external tools and this Project's grants |
| `PUT /v1/projects/{project_id}/external-tools/{tool_id}` | Grant a known external tool to this Project |
| `DELETE /v1/projects/{project_id}/external-tools/{tool_id}` | Revoke a Project's tool grant |
| `POST /v1/projects/{project_id}/external-tools/{tool_id}/invoke` | Invoke a granted typed tool; body is `{ "arguments": { ... } }` |
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
feynman --server URL external-tools invoke PROJECT_ID TOOL_ID --arguments '{"operation":"list"}'
feynman --server URL conversation PROJECT_ID
feynman --server URL chat PROJECT_ID
```

`FEYNMAN_SERVER_URL` may replace the repeated `--server` option. The interactive chat prints streamed deltas and tool activity, and asks before answering a native approval request.

## Security and deployment boundary

The current `DevIdentityProvider` supplies only the local development Principal (`local-user` in Tenant `local`). The server refuses any non-loopback bind until a real identity provider is configured. It does not accept a tenant ID from CLI flags or request bodies. Therefore this is a tenant-isolated local development foundation, not yet a remotely usable multi-tenant service.

The adapter targets Codex CLI 0.157.1. Every thread/resume/turn selects `permissions: "feynman_project"` and native `on-request` approvals. A per-process Permission Profile replaces the removed restricted-read `sandboxPolicy` fields: workspace writes, minimal platform reads and the resolved Codex executable, general filesystem read denial, no temporary-directory writes, and no direct network access. The server injects the profile and Feynman MCP bridge with config overrides; it does not modify global Codex configuration. The bridge and HTTP invocation endpoint expose only fixed, typed operations and check the Project's `Tool Grant` for every invocation. An ungranted call is denied with CLI guidance; after an owner grants access, the Agent can call it in a later turn. See [official Permission Profile documentation](https://learn.chatgpt.com/docs/permissions); validate wire values against the installed CLI's generated schema when upgrading.

The first adapter, `dws.aisearch.enterprise`, invokes only fixed enterprise search with allowlisted arguments. It discovers content without accessing or mutating the matched resource. The updated [External Access module](../modules/external-access/index.html) resolves an explicitly verified Connection and expiring ProjectCliBinding; no host-profile fallback. Credentials stay on the local server, not in the Agent Workspace. First-batch GitLab and Git use fixed read-only CLI operations under the same grant and binding checks. Tool Results include source and observation time, but are not yet persisted as durable Evidence. There is no per-call human approval prompt for these low-risk reads: explicit revocable Tool Grant plus active verified binding is the gate.

Codex model authentication is still supplied by the local Codex profile available to the Server process; provider credential brokering is not implemented yet.

SQLite and the filesystem workspace provider are V1 local adapters. The `PlatformRepository`, `IdentityProvider`, and `WorkspaceProvider` interfaces are the replacement seams for PostgreSQL, user authentication, and a persistent remote sandbox such as GKE Agent Sandbox. The workspace path is derived on the server from Tenant and Project IDs; clients cannot choose arbitrary server paths.

The Codex adapter speaks the App Server JSONL protocol. OpenAI describes App Server as the integration for product-owned persistent conversations, streamed events, tools, and approvals ([Codex App Server overview](https://developers.openai.com/codex/app-server)). The adapter is isolated behind `AgentRuntime`; no Codex-specific IDs appear as the platform's durable identity.
