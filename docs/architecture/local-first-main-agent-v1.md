# Local-first Feynman — Skill + CLI + one Main Agent

Implemented 2026-09-29. Feynman is the local project manager, accessed from the
user's own harness. No GUI or mandatory Reviewer is required.

HTML documentation: [architecture](local-first-architecture.html) and
[local user Home / implementation details](local-user-home.html).

```text
Local Codex / other harness
  [Feynman Skill] --> [feynman CLI] --> [local Rust Server]
                                         |      |      |
                                      SQLite  Tasks  Tool grants / DWS auth
                                         |
                          +--------------+---------------+
                          |                              |
                      [Project A]                    [Project B]
                      [Main Agent]                   [Main Agent]
                      [Codex thread]                 [Codex thread]
                          |                              |
                   native Executor                native Executor
                          |                              |
                   [OpenSandbox A]                [OpenSandbox B]
                   [volume A]                     [volume B]

Reviewer: future optional role; not instantiated or required in V1.
```

## Ownership

Each Project owns one Main Agent identity, Agent Session and Conversation.
Messages, Tasks and Schedule occurrences use that same Conversation. SQLite
persists work, schedules, events, thread IDs and Workspace bindings, scoped by
Tenant/Project. Runtime processes may restart. A database advisory lock prevents
a second daemon from interrupting the first one's Runs or starting another
scheduler; it is not distributed leadership.

The local harness is a manager, not the Project Agent. The implementation reuses
[Codex App Server](https://learn.chatgpt.com/docs/app-server) and
[standalone Skills](https://learn.chatgpt.com/docs/build-skills), without a custom
Skill registry/model harness. Experimental remote messages target tested Codex
0.157.1, rather than assuming protocol stability from documentation alone.

Local workspaces are separate directories. Remote Projects have distinct
containers and retained named volumes. `/workspace` survives replacement; rootfs,
dependencies outside the volume, memory, PTYs and processes do not.

## Local user Home and native sessions

The default root is now `$HOME/.feynman`, independent of the launch cwd.
`FEYNMAN_HOME` overrides it; `FEYNMAN_DATA_DIR` is a compatible alias. Conflicting
roots are rejected. `FEYNMAN_DB` and `FEYNMAN_WORKSPACE_ROOT` remain explicit
overrides. CLI and server share filesystem policy in `feynman-local`.

```text
~/.feynman/
  rust-platform.sqlite3             platform messages/events/tasks/grants
  rust-platform.server.lock         single-daemon database lock
  codex/                            dedicated native CODEX_HOME
    config.toml                     self-owned configuration, not overwritten
    auth.json                       optional explicit import/native login cache
    sessions/                       native transcript JSONL, created by Codex
    ...                             native state databases, archives/logs/caches
  private/dws/                      isolated device-authorization store
  workspaces/<tenant>/<project>/     local Workspace or control cwd
```

The runtime overrides both `CODEX_HOME` and `CODEX_SQLITE_HOME` on each local
app-server child, even if the server inherits personal harness paths. All Projects
currently share the user's native Home, but have separate Thread identities.
This is not per-tenant OS/credential/memory isolation. Host-wide Skills and
provider environment variables are not isolated merely by changing CODEX_HOME.

`home` reads paths offline; `init` creates private directories (`0700`) and a
minimal config (`0600`) without copying personal config/auth/sessions. Existing
public private directories or linked config/auth files are refused without edits.
Optional `init --import-codex-auth SOURCE_HOME` copies only a regular `auth.json`
to a new `0600` cache; identical content is idempotent, differing content is refused.
It never uses a credential symlink. Keyring caches cannot be imported this way.
Source-file writes are isolated, but account-level OAuth refresh may still affect
cache validity. Custom provider configuration must be set independently.

Platform messages remain in SQLite, not a new Feynman JSONL journal. Native Codex
JSONL belongs to the runtime and is resumed through the native API. JSONL export
is reserved, not implemented. Backups must cover both stores, plus separately
back up remote volumes and any overridden local paths. Stop the daemon first or
use consistent backup mechanisms; do not copy only an actively written DB file.

The old cwd `.feynman/` is never deleted/moved. A detected old Rust registry blocks
an implicit switch to a missing default database; explicitly select its root and
restore its matching native history. Missing history causes resume to fail, not
silent Thread creation. There is no automatic session migration or TS `platform.db`
conversion in this slice.

See official [state locations](https://learn.chatgpt.com/docs/config-file/config-advanced),
[credential/device login](https://learn.chatgpt.com/docs/auth) and
[Thread lifecycle](https://learn.chatgpt.com/docs/app-server).

## CLI workflow

```sh
cargo install --path crates/feynman-cli --locked
cargo install --path crates/feynman-server --locked
feynman home
feynman init
CODEX_HOME="$HOME/.feynman/codex" codex login --device-auth
feynman skill install --skills-dir "$HOME/.agents/skills"
feynman-server
```

Use an absolute `FEYNMAN_HOME` for a custom registry; use its printed `codex_home`
for login instead of the default path above. Server binding remains loopback-only. Skill install
is idempotent and refuses to overwrite differing local content or edit global
Codex config. Keep the server running and the machine awake for scheduled work.

```sh
feynman --json status
feynman --json projects create --name "My project"
feynman workspace create PROJECT_ID --provider opensandbox
feynman workspace refresh PROJECT_ID
feynman tasks create PROJECT_ID --title "MVP" --prompt "Goal, constraints, acceptance checks…"
feynman tasks show PROJECT_ID TASK_ID
feynman send PROJECT_ID --message "Explain your plan" --wait
feynman runs PROJECT_ID RUN_ID --wait --timeout-seconds 60
feynman --json conversation PROJECT_ID
feynman chat PROJECT_ID
```

New non-interactive commands return JSON; `--json` also covers older commands.
`runs --after N` reads durable events. Bounded waits return `wait_timed_out: true`
without resubmitting. `runs` returns JSON plus a nonzero exit for failed/interrupted Runs.
Waiting for approval returns immediately; human chat or an explicitly authorized
`approve ... --decision ...` answers the specific pending request. Device auth is
nonblocking in JSON mode, so its verification link can be presented immediately.
Cancellation currently applies to queued Tasks only. Run completion is not proof
of accepted delivery or deployment.

## Schedules and recovery

```sh
feynman schedules create PROJECT_ID --title "Daily check" --prompt "Observe and report; do not deploy" --every-seconds 86400
feynman schedules resume PROJECT_ID SCHEDULE_ID
feynman schedules pause PROJECT_ID SCHEDULE_ID
feynman schedules create PROJECT_ID --title "One report" --prompt "Summarize" --at '2026-09-30T09:00:00+08:00' --enable
```

Plans default to paused. Enablement authorizes future turns, not new tool grants
or automatic approvals. Intervals are 60 seconds to one year; `at` requires an
RFC3339 timezone. With both options, `at` is the first occurrence. Intervals are
delay-based from the scan, not wall-clock cron/calendar recurrences.

Enqueue/advance is one SQLite transaction. Missed intervals coalesce; at most one
pending Task per Schedule is retained. One-shots disable after enqueue. Busy
Projects queue behind their active Run, including approvals; other Projects
dispatch concurrently without backlog starvation. Task claim, Run linkage and
message creation are atomic. Startup interrupts unfinished work while preserving
queued tasks and the thread. Interrupted work is not auto-replayed: external
effects may already exist. This is not exactly-once provider-side delivery.

Pause affects future occurrences; cancel already queued Tasks separately. Running
work is not implicitly killed. Session Heartbeat remains a separate liveness
concept, with no new external heartbeat endpoint in this slice.

## OpenSandbox operator setup

Build on the Docker host using the server's Python environment and an already
extracted `@openai/codex@0.157.1-linux-arm64` (or Linux x64) npm package:

```sh
python scripts/opensandbox/build_executor_image.py --package-dir /path/to/package --dockerfile infra/opensandbox/Dockerfile
```

Store a JSON file outside project files, mode `0600`, and set the local server's
`FEYNMAN_OPENSANDBOX_CONFIG` to its absolute path:

```json
{
  "base_url": "http://127.0.0.1:18080/",
  "api_key": "OPERATOR_SECRET",
  "image": "feynman-codex-executor:0.157.1",
  "executor_ssh_target": "simonlong@spark-d304.tail242882.ts.net",
  "allow_outer_sandbox_only": true,
  "timeout_seconds": 1800
}
```

For Spark, keep an operator-managed lifecycle API forward running:

```sh
ssh -N -L 127.0.0.1:18080:127.0.0.1:18080 simonlong@spark-d304.tail242882.ts.net
```

Feynman owns per-Project Executor forwards, closed with the daemon. Omit the SSH
target for a same-host provider. Only loopback endpoints and direct ports or
`/proxy/44773` are accepted. Redirects, HTTP proxies, credential-bearing URLs and
endpoint auth headers are rejected. The operator must ensure loopback publishing,
dropped capabilities, no-new-privileges, resource limits and no host mounts.
An endpoint string does not prove Docker settings.

OpenSandbox 0.2.3 has no `publish_host` option; smoke uses a scoped in-memory
override, not a daemon/package edit. Durable service setup remains an operator
responsibility. Default public publishing is not secured by an SSH tunnel alone.

Native tools bind a stable Project environment on thread start/resume and every
turn. Local cwd is an app-server control directory. `environment/info` is checked
before starting. Subagent inheritance was proven in the earlier native smoke;
this version does not create a second long-lived Reviewer.

Authorized work restores an owned missing/terminated sandbox onto its retained
volume; ambiguous creation is reconciled by ownership metadata. Network/auth
errors never trigger replacement or host fallback. `workspace restore` also
supports explicit recovery. Paused Workspaces require explicit resume; tasks
wait. Active Runs renew before expiry; idle sandboxes can expire. Mid-command
expiration is not live migration or command replay. `workspace stop` pauses Docker
memory/processes; it is not free scale-to-zero. There is no volume-delete command.

## Security boundary

Restricted Docker cannot nest Codex bwrap. Remote threads use
`:danger-full-access` **inside the outer container**, native `on-request`, and
explicit operator consent. Local mode retains restricted `feynman_project`.
Docker/runc is not gVisor or a VM; shared bridge networking does not isolate
Projects' network trust domains. Neighbor containers can reach unauthenticated
Executor/execd services. Use trusted personal workloads only; dedicated networks,
stronger isolation and authenticated transport are needed before untrusted tenants.

Identity is still fixed local dev, not a remotely accessible multi-tenant service.
Model auth stays in local Codex. Private DWS device auth is not mounted into
containers. Project MCP/grants are preserved; login does not grant business tools.
The image contains Codex/Python, not DWS/Drlee binaries. Native observation adapters
keep credentials on the Server; fixed Sandbox Git clone briefly uses a
credential-bearing process environment. Container root can observe that process:
this is not a hostile-agent secrecy boundary. Approved workload env/tmpfs and
usage leases are documented in [Environment, leases and memory](environment-leases-memory.html).

## External Access module

The independent Rust module provides DWS, GitLab, Git, Slate and Drlee. Connection
registration, project binding and business Tool Grant are separate. DWS search
uses the explicitly verified device cache. Owners may explicitly import native
GitLab login, a selected Slate Keychain profile or a single Drlee account. Slate
browser PKCE and Drlee native browser login remain their actual protocols, not
DWS device flows. Observation CLIs execute fixed reads on the local control plane.
Git delegates its HTTPS Connection for `ls-remote` and fixed Sandbox-only clone;
no authenticated fetch/push or SSH. Bindings expire (default 24h); revocation,
disable and identity changes block new calls. Agent MCP can request access, not
approve it. The [2026-10-09 real smoke](../modules/external-access/index.html#real-smoke)
verified native authorization, Agent reads, Sandbox clone and three local config
tests; it did not test a full business environment. See
[External Access architecture and CLI](../modules/external-access/index.html).

## Verification

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
python3 -m unittest discover -s scripts/smoke -p test_project_manager.py -v
python3 -m unittest discover -s scripts/smoke -p test_dws_authorization.py -v
python3 -m unittest discover -s scripts/smoke -p test_external_access.py -v
python3 scripts/smoke/run_project_manager_smoke.py
```

The last command contacts the real provider and Spark. It creates two test
Projects/sandboxes, verifies remote write, separate volumes, pause/resume,
replacement and the same Main Agent thread after daemon restart. It cleans its
explicit test containers/volumes and temporary service; image caches/evidence stay.
Routine integration tests substitute upstream Codex/OpenSandbox only, not Feynman.

Real smoke passed twice on 2026-09-29. The final expanded run took **82.76 seconds**
(whole test, not cold start), including automatic recovery on the live harness
and a real one-shot scheduled task. The earlier smaller run took 34.73 seconds,
using Codex 0.157.1, OpenSandbox 0.2.3 and image
`sha256:eaf7a7a242e05d30ba51681c8b1fa829054a2f555a4b2cb34631e2fc3cd7624b`.
Final local evidence: `feynman-project-manager-smoke-wxqizb2g` in temporary storage;
earlier evidence: `feynman-project-manager-smoke-d8n1rit6`.
Lifecycle/volumes were checked against the
[official contract](https://github.com/opensandbox-group/OpenSandbox/blob/main/specs/sandbox-lifecycle.yml)
and installed 0.2.3. See [OpenSandbox architecture](https://open-sandbox.ai/architecture/)
for the distinct guarantees of pause and retained-volume recovery.
