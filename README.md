# Feynman

Feynman is a local-first project manager driven by **Skill + CLI** from the user's own Agent harness. Each Project has one persistent Main Agent, a work queue, schedules, external-tool grants and a persistent Workspace. Reviewer is reserved for later, not required. The Rust server owns records and scheduling; execution can use local directories or remote OpenSandbox containers through Codex's native Executor.

Open [docs/index.html](docs/index.html) for the documentation site and clickable SVG architecture,
[user Home and implementation details](docs/architecture/local-user-home.html),
and [operator setup](docs/architecture/local-first-main-agent-v1.html).
The overview shows one Feynman managing multiple Projects. See the
[external access abstraction design](docs/architecture/external-access-abstraction.html)
for Connection ownership, per-Project grants, permission requests and proposed
Rust Provider traits; the initial Broker and Connection-bound reads are implemented.
All reading pages share the same OpenAI-inspired theme, navigation and light/dark
mode. They work directly from local files without a web server or frontend build.
Markdown and DOCX are retained as source/history, not the canonical reading UI.

To regenerate the historical HTML pages after source edits (requires Pandoc):

```sh
python3 scripts/docs/build_html.py
python3 scripts/docs/build_html.py --check
python3 -m unittest discover -s scripts/docs -p 'test_*.py'
```

## Build and test

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Run locally

### Local Console

The Rust server now serves a usable project-first browser workbench at `/console/`.
For the existing local Docker testing Home, open **http://127.0.0.1:4318/console/**;
the default server uses **http://127.0.0.1:4317/console/**. No separate Node server,
frontend build, CDN or GUI credentials are needed. HTML/CSS/native JavaScript are
embedded in the Rust binary, so rebuild/restart an idle server after UI changes.

The Console supports project creation, Main Agent conversation through durable
Inbox, native approval cards, Agent settings, tasks, paused-by-default Schedule
creation, source registration and independent live windows, Workspace lifecycle,
existing Connection binding and project Tool Grants.
The home page has a dedicated provider capability unit and a read-only system
catalog. Project bindings/grants and observation channels are grouped by source;
provider filters survive reloads. The Rust catalog distinguishes implemented
tools from managed collection: DWS supports history/event sources; the other
providers currently support on-demand tools only. Viewing never starts auth,
subscribes a source or grants a capability.

Memory/environment/lease
views are read-only; credential import and native Slate/Drlee login remain CLI
operations. It does not provide a file browser, multi-Agent sessions, raw secret
registration, arbitrary CLI execution or unsafe delivery replay.

The browser adapter requires exact loopback Host, same-origin writes and an
in-memory per-server CSRF token, plus CSP/frame protection. It routes only an
allowlisted set of public owner operations to the existing handlers. CLI APIs
continue rejecting Origin headers; no permissive CORS is added. This is still a
trusted single-user local service, not network authentication or tenant isolation.
Reload the Console after a server restart; writes are never automatically retried.
See [Console design and current scope](docs/architecture/visual-console.html).

### CLI and local Docker

To use **a local Docker sandbox instead of a host directory or Tailscale**, see
[local Docker Workspace](docs/architecture/local-docker-workspace.html).
The operator helper starts an isolated testing Home and loopback control processes:

```sh
python3 scripts/opensandbox/local.py prepare --import-codex-home /absolute/company-feynman-codex-home
python3 scripts/opensandbox/local.py up
python3 scripts/opensandbox/local.py cli -- workspace create PROJECT_ID --provider opensandbox
```

No remote registry/volume or DWS/business credentials are copied implicitly.
`--provider local` below remains an unsandboxed host directory, not a container.

In one terminal:

```bash
cargo run -p feynman -- home
cargo run -p feynman -- init
cargo run -p feynman -- auth login --device
cargo run -p feynman -- auth status
cargo run -p feynman-server
```

The server listens on `127.0.0.1:4317`. Its default user Home is `$HOME/.feynman`, independent of the working directory: `rust-platform.sqlite3` holds platform records, `codex/` is its dedicated Codex Home, and `workspaces/<tenant-id>/<project-id>` holds local project/control directories. `FEYNMAN_HOME` selects another root; legacy `FEYNMAN_DATA_DIR` remains a root alias (conflicting values are rejected). `FEYNMAN_DB` and `FEYNMAN_WORKSPACE_ROOT` remain explicit overrides. New private directories use `0700`; existing public/linked private directories are refused, not silently chmodded.

`home` is read-only and `init` works without a server. Neither implicitly copies personal credentials, configuration or sessions. To explicitly import a regular file-based auth cache instead of logging in, use `feynman init --import-codex-auth /absolute/source/codex-home`. This copies only `auth.json` into a private `0600` file, never symlinks it or overwrites a different cache. For a custom Feynman Home, use the `codex_home` path shown by `feynman home` for native login. Cache presence is not proof of valid authentication.

In another terminal:

```bash
cargo run -p feynman -- projects list
cargo run -p feynman -- --json status
cargo run -p feynman -- projects create --name "My project"
cargo run -p feynman -- chat PROJECT_ID
cargo run -p feynman -- external-tools list PROJECT_ID
cargo run -p feynman -- external-tools grant PROJECT_ID dws.aisearch.enterprise
cargo run -p feynman -- external-tools revoke PROJECT_ID dws.aisearch.enterprise
cargo run -p feynman -- external-tools invoke PROJECT_ID dws.aisearch.enterprise --arguments '{"queries":"example","types":["document"]}'
```

Set `FEYNMAN_SERVER_URL` to use another server URL. The server launches `codex app-server` on the first chat turn for a project; Codex must be installed and authenticated in Feynman's dedicated Home. The child explicitly sets both `CODEX_HOME` and `CODEX_SQLITE_HOME` to that directory, overriding inherited personal harness paths without changing the parent shell. `FEYNMAN_CODEX_BIN` and `FEYNMAN_CODEX_MODEL` can select its executable and model. Custom provider configuration belongs in Feynman's own `codex/config.toml`.

Old working-directory state is untouched. If the new default database is absent and an old Rust registry is found, the server refuses an implicit empty-registry switch; explicitly select the old root and restore its native Codex session history. A missing stored Thread fails rather than starting a new conversation. There is no automatic cross-Home session or old TS `platform.db` migration. Back up platform SQLite and the entire dedicated Codex Home together; remote volumes need a separate backup.

The Codex process receives a project-scoped Feynman MCP server through per-process config overrides; Feynman does not edit the user's global Codex configuration. The first external tool is `dws.aisearch.enterprise`, a fixed read-only DWS enterprise search. List tools and grant one to a Project with the commands above; the MCP bridge checks the grant on every call. A missing grant causes the call to be denied, and an owner can grant it before a later Agent turn. The tool is intentionally not an arbitrary DWS command or shell interface.

## Company model profile and frozen-repository evaluation

The current local operator profile uses the company credential cache imported
explicitly from `~/.codex-company`, with `gpt-5.6-terra` and `max` reasoning.
No personal auth/config/history is imported. For a native file-based API-key
profile, configure its company Responses gateway in Feynman's owned Home with
`requires_openai_auth = true`, without `env_key`; native Codex then uses the
owned `auth.json`. The runtime removes inherited `OPENAI_API_KEY`,
`CODEX_API_KEY` and `CODEX_ACCESS_TOKEN`. Do not put keys in argv, prompts or
images. Model authentication is not network Feynman user authentication:
this MVP remains loopback-only with a fixed local Principal.

```sh
feynman evaluation prepare --repository /absolute/repository \
  --base-sha FULL_COMMIT_SHA --facts /private/frozen-facts.json \
  --output /private/new-snapshot.json
feynman workspace seed PROJECT_ID --input /private/new-snapshot.json
```

Preparation exports only tracked files from that exact SHA. It does not copy
the original Git database, remotes, future refs or dirty worktree files.
Symlinks, submodules, LFS pointers and credential-like filenames require explicit
exclusion or separate audited preparation. `--exclude-prefix` is repeatable;
all exclusions are recorded. Source plus facts are bounded to 32 MiB. Facts
require a timezone-qualified cutoff; message `created_at` must not exceed it.
These checks do not reconstruct historical edits or audit arbitrary attachments.

Seeding creates `repo/`, `facts/`, `scratch/`, `output/` and a seed manifest in
an unused local/OpenSandbox Workspace. It refuses previous Runs and existing
target content. The fresh repository has one initialization commit, whose SHA
differs from the original source SHA. Docker currently runs as root: chmod is
advisory, not enforced read-only isolation. The operator must independently
verify source/facts hashes, file sets, Git state and credential absence before
and after an evaluation. See [test design](docs/testing/iteration-direction.html).

Opt-in real validation uses `python3.13 scripts/smoke/run_mvp_smoke.py` with
explicit company source Home, prepared fixture directory and DWS binary. It
requires the user's device consent and paid model calls, creates only disposable
Spark containers/volumes, and removes those resources on completion. Private
messages/proposals stay outside the tracked docs. Protocol success and semantic
proposal quality are recorded separately; two curated samples are not a full
self-maintenance benchmark.

## Skill + CLI delegation

```sh
cargo install --path crates/feynman-cli --locked
feynman skill install --skills-dir "$HOME/.agents/skills"
feynman --json projects create --name "My project"
feynman workspace create PROJECT_ID --provider local
feynman tasks create PROJECT_ID --title "MVP" --prompt "Implement the goal and verify acceptance checks"
feynman tasks list PROJECT_ID
feynman send PROJECT_ID --message "Explain your plan" --wait
feynman schedules create PROJECT_ID --title "Daily check" --prompt "Summarize changes" --every-seconds 86400
feynman schedules resume PROJECT_ID SCHEDULE_ID
```

Schedules are paused by default and require the server to run while the machine
is awake. Missed occurrences coalesce; busy Projects queue work. Interrupted Tasks
are not automatically replayed. `--json` supports automation and never answers
approvals implicitly. Existing modified Skill content is not overwritten.

## DWS device authorization

An Agent can request device login through `feynman_dws_authorization` after the
owner grants `dws.auth.device`. The server runs isolated DWS authentication,
returns the verification link/code through project-scoped HTTP, and checks the
completed identity without returning tokens. Chat watches progress independently
of the model turn; a future frontend can poll the same HTTP record.

```sh
cargo run -p feynman -- external-tools grant PROJECT_ID dws.auth.device
cargo run -p feynman -- dws-auth request PROJECT_ID --reason "Read project facts" --wait
cargo run -p feynman -- dws-auth status PROJECT_ID AUTHORIZATION_ID --wait
```

See the Chinese [DWS CLI guide](docs/modules/external-access/dws.html) and
[shared authentication/authorization reference](docs/reference/external-authentication-flow.html).
Authentication stays on the local control plane, not in OpenSandbox. Register a
verified authorization as a Connection and explicitly bind it to the project;
search no longer silently falls back to the host profile.

```sh
feynman access connections register-dws PROJECT_ID AUTHORIZATION_ID
feynman access bind PROJECT_ID --provider dws --connection CONNECTION_ID
feynman external-tools grant PROJECT_ID dws.aisearch.enterprise
```

## External Access: DWS, GitLab, Git, Slate and Drlee

For exact chat-history reads, the owner additionally configures a project
source/time boundary, then grants `dws.chat.messages.read`:

```sh
feynman access dws-scope PROJECT_ID --scope-json '{"targets":[{"kind":"group","id":"STABLE_GROUP_ID"}],"start":"2026-09-01T00:00:00+08:00","end":"2026-09-02T00:00:00+08:00"}'
feynman external-tools grant PROJECT_ID dws.chat.messages.read
```

MCP `dws_read_messages` accepts only one configured target and a sub-window.
There is no arbitrary shell/CLI argument interface or attachment download.
All DWS business calls explicitly pin the verified profile. This scope restricts
the new chat tool, not the separate enterprise-search capability.

The independent Rust module manages verified Connections, project bindings,
24-hour deduplicated requests, binding expiry, local disable and minimal audit.
Business Tool Grants remain separate. Each call revalidates identity and current
binding; Project A authentication does not grant Project B access.

```sh
feynman access providers
feynman access connections list
feynman access status PROJECT_ID
# Owner-only input: supply stdin from a trusted secret-manager pipe, not argv/chat.
# This reads until EOF; it is NOT a hidden interactive password prompt.
feynman access connections register-gitlab --hostname gitlab.example.com --token-stdin
feynman access bind PROJECT_ID --provider gitlab --connection CONNECTION_ID --repository team/repo
feynman external-tools grant PROJECT_ID gitlab.repo.read
feynman external-tools grant PROJECT_ID gitlab.mr.read
feynman external-tools invoke PROJECT_ID gitlab.repo.read --arguments '{}'
feynman access bind PROJECT_ID --provider git --connection CONNECTION_ID --repository team/repo
feynman external-tools grant PROJECT_ID git.remote.read
feynman external-tools invoke PROJECT_ID git.remote.read --arguments '{}'
feynman access unbind PROJECT_ID --provider git
feynman access connections disable CONNECTION_ID
```

GitLab uses native `glab api` with an owner-provided token or explicit
`access connections import-gitlab --hostname HOST` from the native glab login.
Git delegates HTTPS credentials to that Connection for fixed `ls-remote` and
`git.workspace.clone` (MCP `git_workspace_clone`), which clones only the bound
repository into `/workspace/repo` in a running OpenSandbox. Clone requires a UUID
idempotency key; it never replaces existing repository data. Personal helpers,
redirects, templates and hooks are disabled. No arbitrary argv, SSH, push, MR
writes or pipelines. Native Agent shell tools can develop/test the cloned tree.
Only one account/repository binding per provider/project; default TTL is 24h
(60 seconds–7 days). Reads use Server Proxy; clone credentials enter only the
fixed Sandbox Git process via stdin-to-process-env transport, never a persistent
repo/URL/image. A trusted root Agent can inspect process env; this is not hostile
agent isolation. Set `FEYNMAN_GLAB_BIN` / `FEYNMAN_GIT_BIN` if needed.

Slate uses its actual Python CLI package with an in-memory session store, native
browser PKCE (`login-slate`) or explicit Keychain/session import (`import-slate` /
`register-slate --token-stdin`). It does not have device-code support. Drlee uses
`login-drlee` or copies exactly one account with `import-drlee`; refresh updates
only Feynman's private `DRLEE_HOME`. Owner hostname is explicit and HTTPS-only.
The Agent receives read results, not credentials or a mutable login directory.
Drlee messages require an owner-bound channel UUID (`--resource-id`).

```sh
# Local Docker operator setup; installs Slate only in Feynman's dedicated venv.
python3.13 scripts/opensandbox/local.py prepare-clis --slate-package /path/to/slate/cli
# Restart the owned Feynman server when idle to load native-cli.json.
python3.13 scripts/opensandbox/local.py cli -- access connections login-slate --hostname sh.slate.star-x.dev
python3.13 scripts/opensandbox/local.py cli -- access connections import-drlee --hostname drlee.dip-aitech.com
```

For other operator setups, select `FEYNMAN_NATIVE_PYTHON` (installed dip-slate
package) and `FEYNMAN_DRLEE_BIN`. First tools: Slate identity/mine tasks/projects;
Drlee identity/channels/bound-channel messages/personal knowledge metadata. No
business writes. See the [Slate](docs/modules/external-access/slate.html) and
[Drlee](docs/modules/external-access/drlee.html) authentication modules.

After an owner grants `external.access`, project MCP `feynman_access` supports
only `providers`, `status`, `request`; it cannot register secrets, bind or approve.
Owners handle a request with Connection registration/reuse, explicit binding and
the existing `external-tools grant` command. Requests do not start real login.
Configured status is metadata, not proof of current live authentication.

Private `external-access/access.sqlite3` and 0600 token files live under the
0700 `~/.feynman/private/` tree. Tokens are private plaintext, not an encrypted
vault. Back up this private state and DWS caches securely alongside platform
SQLite and Codex Home. Local disable is not provider token revocation.
See [External Access module](docs/modules/external-access/index.html).

## Current security boundary

This development server deliberately has no user-login flow yet. Its identity provider exposes only a fixed local Tenant and refuses a non-loopback bind. Do not expose it to a network. Persistence is tenant-scoped; callers cannot select a Tenant through a request body. Production authentication, provisioning, PostgreSQL and an encrypted credential vault remain follow-up work. Credentials/caches are not mounted into Agent Workspaces; fixed Sandbox clone briefly uses process credentials as described above. Set `FEYNMAN_DWS_BIN` if needed. Management CLI and project MCP are protocol boundaries, not strong isolation between processes running as the same OS user.

The runtime targets Codex CLI 0.157.1. Local execution uses `on-request` and the per-process restricted `feynman_project` profile. Remote Docker execution explicitly requires `allow_outer_sandbox_only`: native tools bind only the remote environment, using `:danger-full-access` inside the outer container because restricted Docker cannot nest bwrap. This is trusted personal testing, not strong untrusted multi-tenant isolation; shared bridge networking and unauthenticated Executor transport need hardening. No global Codex configuration is changed. Feynman MCP checks project grants; model authentication stays local.

## OpenSandbox + Docker smoke

An opt-in diagnostic Rust adapter exposes `sandbox.files.smoke` through the same
project grant checks, HTTP invocation route and MCP bridge. It lists and reads
only two dummy files in one operator-assigned sandbox. It is **not** the remote
Agent Runtime or production WorkspaceProvider. The repeatable smoke creates a
Spark Docker sandbox, verifies real CLI/MCP reads, replaces the container and
reattaches its named volume, then removes its test resources automatically.
See [smoke instructions](docs/testing/smoke-guide.html#opensandbox-docker-read-smoke).

A separate [native Executor smoke](docs/testing/smoke-guide.html#native-codex-executor-separation-smoke)
verified a local Codex app-server with native Shell/apply-patch and two subagents
executing inside the remote container, plus host-canary isolation and recovery.
See [measured results and security limits](docs/architecture/native-executor-smoke-2026-09-28.html).
The Rust product now supports the same native remote path through its Project
WorkspaceProvider. Its [manager smoke](scripts/smoke/run_project_manager_smoke.py)
verifies real CLI task dispatch, separate Project volumes, pause/resume, container
replacement and the same Main Agent thread after server restart.

The interactive CLI resumes active Runs and prompts for native approvals.
Scheduled Project tasks are now supported; Session Heartbeat remains separate
liveness and has no new external endpoint.

See [backend and CLI contracts](docs/architecture/backend-cli-v1.html) for routes, boundaries, and the heartbeat distinction. The original architecture source document remains in [`docs/reference`](docs/reference/).
