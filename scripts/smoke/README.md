# GKE Sandbox read smoke

## External Access V1 offline regression

```sh
cargo build --workspace
python3 -m unittest discover -s scripts/smoke -p 'test_external_access.py' -v
```

This suite exercises compiled Rust HTTP, CLI and MCP with fake dws/glab/git,
temporary private state and fake credentials. It creates no cloud resources,
contacts no real external account and makes no model calls. It covers verified
DWS cache binding, explicit shared-account grants, GitLab repository/MR reads,
native Git askpass destination checks, TTL, revoke/disable, cross-project denial,
stdin registration, unsafe argument rejection and public secret redaction.
For actual owner commands, see
[External Access module](../../docs/modules/external-access/index.html).

## Historical GKE read fixture

Diagnostic fixture, **not** Feynman's production GKE WorkspaceProvider. The Agent
runs on the local host; the workspace runs inside a remote GKE Agent Sandbox.
Python is used only for this dependency-free test harness; the backend remains Rust.

The test creates a random marker inside the sandbox, never puts it in the Agent
prompt, then compares the Agent's structured answer against the independent
verifier. Both native Codex MCP events and successful remote read audit records
must exist. Local command tools, other MCP servers, plugins and browsers are
disabled for the test; Code Mode's tool invocation host must remain enabled.

The bridge exposes directory listing and two allowlisted file reads only. It
cannot accept shell commands, choose another Pod, or read another path. The host
uses a short-lived ServiceAccount token scoped to one Pod; the sandbox receives
neither host/cloud credentials nor an automatically mounted Kubernetes token.
Kubernetes `pods/exec` RBAC itself is **not** a read-only filesystem permission:
the fixed-command MCP implementation supplies that restriction. This fixture is
not a production security boundary or a multi-tenant isolation audit.

## Run

Use a **disposable** GKE cluster with managed Agent Sandbox enabled, and an
explicit admin kubeconfig outside the repository. This fixture uses the fixed
namespace `feynman-smoke`; do not run it against an existing user namespace.
Use a kubectl version supported by the cluster, and existing local Codex auth.

```sh
python3 -m unittest discover -s scripts/smoke -p 'test_gke_sandbox_mcp.py' -v
SMOKE_RUN_DIR="$(mktemp -d /tmp/feynman-gke-read.XXXXXX)"
python3 scripts/smoke/run_gke_sandbox_read_smoke.py \
  --admin-kubeconfig /absolute/path/to/disposable-admin-kubeconfig \
  --scratch "$SMOKE_RUN_DIR"
```

`verification.json` records the result without credentials or the raw marker.
The temporary reader kubeconfig is removed on completion/failure; logs and the
dummy-file Agent answer remain in the scratch directory, never in the repository.
Use a new scratch directory after each Agent run. Failed diagnostic runs return
a nonzero exit code; they must not be reported as a passing smoke.

Provisioning and cloud resource cleanup are deliberately **not automatic** in
this runner. The operator must delete the fixture namespace, wait for its PVC
disk to be deleted, then clean up the disposable cluster and any dedicated
network/IAM resources. Deleting a cluster alone does not demonstrate disk cleanup.
The checked-in fixture uses `operatingMode: Running`, confirmed against the live
GKE `v1beta1` schema; older examples using `replicas` are not interchangeable.

This smoke does not test sandbox pause/recovery, long-lived Agent sessions,
Feynman's CLI-to-server integration, Generator/Reviewer review, or deployments.

## OpenSandbox + Docker read smoke

This separate fixture uses **no GKE resources**. The backend and CLI are Rust;
Python is only the operator's provision/verification harness. It exercises:

```text
Rust CLI ──HTTP──> Feynman Server ──SSH tunnel──> OpenSandbox Server
                       │                              │
                 Project grant                  Docker container
                       │                         /workspace volume
Local Main Agent ──MCP─┘                              │
                                            smoke/probe.json, note.txt
```

Prerequisites: existing SSH access and trusted Docker access on Spark, Python
3.10+ with `venv`, local Rust binaries, and (only for `--agent`) an authenticated
local Codex CLI **0.157.1**. Do not share the remote Docker daemon with untrusted
tenants. The test defaults to `simonlong@spark-d304.tail242882.ts.net` and reserves
remote loopback port `18080`; use `--remote-port` if it is already occupied.

Prepare an isolated venv on the remote host, not a global installation:

```sh
ssh simonlong@spark-d304.tail242882.ts.net 'mktemp -d /tmp/feynman-opensandbox-runtime-XXXXXXXX'
# Substitute the exact directory returned above for REMOTE_RUNTIME_DIR below.
ssh simonlong@spark-d304.tail242882.ts.net 'python3 -m venv REMOTE_RUNTIME_DIR/venv'
ssh simonlong@spark-d304.tail242882.ts.net 'REMOTE_RUNTIME_DIR/venv/bin/python -m pip install opensandbox-server==0.2.3'
ssh simonlong@spark-d304.tail242882.ts.net 'docker pull sandbox-registry.cn-zhangjiakou.cr.aliyuncs.com/opensandbox/execd:v1.0.22'
# Supply a trusted ARM64-compatible python:3.12-slim image on the host as well.
```

The run on 2026-09-28 retained a prepared venv at
`/tmp/feynman-opensandbox-smoke.tGx9qM` on Spark, plus the image cache. It can be
reused until the host cleans `/tmp`; it is not a service installation.

```sh
cargo build --workspace
OPENSANDBOX_SMOKE_DIR="$(mktemp -d /tmp/feynman-os-smoke-XXXXXX)"
python3 scripts/smoke/run_opensandbox_smoke.py \
  --remote-runtime-dir /tmp/feynman-opensandbox-smoke.tGx9qM \
  --scratch "$OPENSANDBOX_SMOKE_DIR"
```

Add `--agent` to also send a real message through Feynman's Main Agent endpoint.
The prompt never contains the random marker. The harness independently seeds
and reads files with operator-only Docker access, then compares CLI/MCP/Agent
results. The Agent test requires native sandbox MCP tool activity and rejects
shell activity; it uses the server process's existing model authentication.

The test verifies list/read, grant-before-use, revoke denial, path traversal
denial, and rejection of another Project even when that Project grants the same
tool. It deletes the container, creates a replacement attached to the same named
volume, rebinds the diagnostic connection and restarts Feynman, then reads again.
This is **file persistence**, not live-process checkpointing or automatic Session
recovery. Warm-cache startup timings exclude image downloads and service setup.

`verification.json` contains timings, pass flags and a marker hash; logs and
optional Agent answer/events stay in the scratch directory outside the repo.
Success and failure both attempt to stop the test server/tunnel, remove only the
fixture-labelled containers and named volume, and delete connection/key files.
Remote fixture logs/helper/manifest, venv and image caches remain inert. Existing
containers are not modified. Review `cleanup` in the report before claiming a
fully clean run; the runner does not perform a Docker prune or restart.

### Opt-in diagnostic adapter

`FEYNMAN_OPENSANDBOX_SMOKE_CONFIG` points to a private (`0600`) operator-managed
JSON file with `base_url`, `api_key`, `tenant_id`, `project_id`, `sandbox_id`.
The base URL must be `http://127.0.0.1:PORT` (the SSH tunnel). No config means no
additional tool. The server forwards only the **config path**, not the key, into
its project-scoped MCP process using [Codex's documented STDIO environment
configuration](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).

```sh
feynman external-tools grant PROJECT_ID sandbox.files.smoke
feynman external-tools invoke PROJECT_ID sandbox.files.smoke --arguments '{"operation":"list"}'
feynman external-tools invoke PROJECT_ID sandbox.files.smoke --arguments '{"operation":"read","file":"probe.json"}'
```

The tool never accepts sandbox IDs, URLs, host paths or commands from callers.
It fixes the root to `/workspace/smoke`, permits only two filenames, caps responses
at 64 KiB, disables redirects/proxies, and returns generic upstream errors.
The OpenSandbox key has lifecycle privileges and remains in the trusted broker;
the file-tool restriction is enforced by Feynman, not by provider read-only RBAC.
It is not a multi-tenant security audit, symlink-safe general filesystem gateway,
credential vault, or production WorkspaceProvider. No host credentials, Docker
socket or GPU are mounted into the sandbox.

The downloaded 1.1.0 wheel failed at startup with a missing
`fast_sandbox.generated` module, so this fixture pins working server 0.2.3.
That version publishes ports on `0.0.0.0` by default. The operator helper applies
a version-checked **in-memory loopback-only override** to its port allocator;
it does not modify installed package files or Docker daemon configuration.
Inspect asserts that every test-container port is bound to `127.0.0.1`.
Use an upstream-supported loopback publish setting when replacing this fixture
with production deployment. [Official image registries](https://github.com/opensandbox-group/OpenSandbox#official-container-images).

## Native Codex Executor separation smoke

This is **not** the read-only MCP bridge above. Codex app-server stays on the
local host, while native command execution and apply-patch run through a Codex
`exec-server` **inside the remote OpenSandbox container**. Subagent orchestration
is native Codex orchestration. No Rust product API is changed by this diagnostic.

The agreed public test seam is app-server's environment/thread/turn protocol.
The real acceptance tests require operator SSH/Docker access and existing local
Codex authentication. They are opt-in and skipped in ordinary unit-test runs.

```text
Local app-server + durable thread history
        | native execution protocol over loopback SSH tunnel
        v
Spark / OpenSandbox Docker container / codex exec-server
        |
        +-- /workspace persistent named volume
```

Prerequisites: the previous fixture's isolated OpenSandbox 0.2.3 venv and images,
and **Codex 0.157.1 on both ends**. On Spark, the verified official ARM64 package
is currently unpacked at `/tmp/feynman-native-executor-LobzkDCF/package`.
It is an inert package cache, not a running service. The original npm tarball is:

`https://registry.npmjs.org/@openai/codex/-/codex-0.157.1-linux-arm64.tgz`

Its npm `dist.integrity` SHA-512 value was verified before unpacking:

```text
sha512-nEaBZT3ldrtqUNaBlvI+Y+fg+LbNCTsQjbUnl7Q45rl9vSSqHU2vRj1l6iJDwupZkrpeIJ/ClHIwxgMKv1SKHg==
```

If `/tmp` has been cleaned, download and verify the versioned package into a new
operator-owned temporary directory and pass `--remote-package-dir /exact/path/package`.
Do not install a global CLI on Spark or mount the package directory into the
container. The trusted fixture copies the binaries into the container's `/opt`.

```sh
NATIVE_SMOKE_RUN_DIR="$(mktemp -d /tmp/feynman-native-smoke-XXXXXXXX)"
python3 scripts/smoke/run_native_executor_smoke.py \
  --scratch "$NATIVE_SMOKE_RUN_DIR" --stage recovery
```

Stages are cumulative: `main`, `subagents`, `isolation`, `recovery`. The full
`recovery` stage verifies all four requested acceptance points:

1. Main Agent native Shell observes Linux, container hostname, `/workspace`,
   and an independently seeded random marker; native apply-patch fixes a
   deliberately broken addition function and native unittest passes.
2. Two native subagents inherit the same environment, independently read it,
   and write separate result files. Their own thread histories contain remote
   command execution; no custom agent dispatcher or MCP command wrapper is used.
3. Native commands cannot read or modify dummy canaries on either physical host.
   The independent operator verifies those files are unchanged. This tests normal
   execution placement, not a hostile container-escape security audit.
4. Dropping the SSH tunnel yields a disconnected environment and failed remote
   command, not successful local fallback. Reconnecting, restarting exec-server,
   replacing the container with the same volume, and restarting app-server then
   resuming the **same thread ID** each allow fresh remote reads and tests. A
   conversation-only nonce proves that session context is retained independently
   of the workspace files.

Run the opt-in integration tests individually or together; they provision
separate temporary fixtures sequentially:

```sh
FEYNMAN_RUN_NATIVE_EXECUTOR_SMOKE=1 \
  python3 -m unittest discover -s scripts/smoke -p 'test_native_executor_smoke.py' -v
```

The harness uses a private local runtime home with a temporary link to the local
profile's `auth.json`; that link is removed at shutdown. No model login file,
OpenSandbox key, Docker socket, host directory, or GPU is mounted in the sandbox.
Native exec-server needs no model authentication for this direct connection.
All MCPs, plugins, browser/computer tools, and host skill discovery are disabled.

**Important security limitation:** Docker's default restrictions prevent the
inner Codex `bwrap` helper from creating a user namespace. The test therefore
explicitly selects `:danger-full-access` **inside the bound remote environment**
and relies on unprivileged Docker/runc, dropped capabilities, no bind mounts,
and no-new-privileges as the outer boundary. This is not a production multi-tenant
configuration or gVisor. The native executor port is not published; SSH forwards
to its container bridge address. Its unauthenticated listener must not be exposed
to untrusted hosts or workloads on the bridge network.

Recovery is operator-driven re-registration and re-binding, not automatic
Feynman recovery, process checkpointing, or restoration of arbitrary changes to
the container root filesystem. Only `/workspace` lives on the persistent volume.
Client-facing app-server `fs/*`/`process/*` APIs are not asserted to become remote
merely because an Agent thread is bound to a remote environment.

Success and failure attempt to stop the local app-server, SSH tunnels, remote
OpenSandbox service, and fixture-labelled containers; the test named volume is
removed. Cache packages, fixture helpers/logs, dummy canaries, and private local
conversation evidence remain inert. Inspect `verification.json` and actual
listeners/container state before claiming cleanup. No Docker prune/restart or
GKE resource is involved. See the [measured result](../../docs/architecture/native-executor-smoke-2026-09-28.md).

## DWS 设备登录测试 {#dws-device-authorization}

离线集成测试：运行真实 Rust HTTP / MCP / CLI，仅替换外部 dws 进程，不执行真实登录：

```sh
cargo build --workspace
python3 -m unittest discover -s scripts/smoke -p test_dws_authorization.py -v
```

真实 Provider 测试需显式选择运行：使用隔离缓存，用户必须自行完成浏览器确认：

```sh
python3 scripts/smoke/run_dws_device_smoke.py --dws /absolute/path/to/dws --wait-seconds 90
```

到期时脚本取消未完成认证，并停止自己启动的 Server；证据保留在输出的私有临时目录。
拿到验证链接不等于成功登录，假凭证的成功测试也不证明真实用户已同意。
中文操作、专属状态和历史结果见
[DWS CLI 验证记录](../../docs/modules/external-access/dws.html#verification)；
通用规则见 [公共认证与授权流程](../../docs/reference/external-authentication-flow.html)。

## Local Console browser adapter

The local Docker launcher retains the host Codex (and code-mode companion when
provided) in its own versioned runtime directory. The offline setup tests delete
the upstream package-manager executable and then run a real Rust/CLI turn against
the retained fixture. They also cover version/hash drift and refusing startup
when HTTP health is green but Codex is missing. For an existing stale Home,
stop idle control services, run `local.py repair-codex`, then `local.py up`;
repair keeps the pinned executor version and does not touch model credentials.
The browser fixture additionally verifies that a failed, empty assistant reply
shows its error in the conversation and never triggers automatic resubmission.

```sh
cargo build --workspace
python3 -m unittest discover -s scripts/smoke -p test_console.py -v
```

The compiled Rust server serves its embedded browser UI at `/console/`.
These public-interface regressions cover Host / Origin / CSRF, excluded secret
and arbitrary-execution endpoints, unchanged CLI Origin rejection, project/source
creation, paused Schedule, Inbox chat, native approval and same-thread recovery
across a server restart. Only upstream Codex is substituted; no model is called.

`console_browser.cjs` uses an explicitly selected installed Playwright module
and a fresh headless Chrome profile. Mutation mode requires
`FEYNMAN_CONSOLE_FIXTURE=true` plus an isolated fixture URL. Do **not** enable it
for a real user server. `--preview` is read-only and may inspect a real project;
`FEYNMAN_CONSOLE_URL`, `FEYNMAN_CONSOLE_PREVIEW_PROJECT`, and
`FEYNMAN_PLAYWRIGHT_MODULE` select its inputs. Screenshots are generated under
`output/`; no external messages, native login, push or deployment are performed.

## Native CLI authorization and Sandbox clone

Offline regressions run the real Rust HTTP / CLI / MCP and fixed native bridges,
substituting upstream Slate / Drlee / Git / Docker. They do not authenticate real
accounts or test hostile-container isolation:

```sh
cargo build --workspace
python3 -m unittest discover -s scripts/smoke -p test_native_access.py -v
python3 -m unittest discover -s scripts/smoke -p test_workspace_clone.py -v
```

Coverage includes explicit selected-account import, private independent refresh,
identity changes during a read, project binding and Grant checks, channel UUID
scope, secret scrubbing, Agent request without owner approval, pinned HTTPS
askpass, Sandbox-only clone, repeat-key recovery and refusal to overwrite an
existing worktree. The clone tests never contact GitLab.

The separately owner-approved 2026-10-09 real local Docker smoke used company
model credentials and verified GitLab / Slate / Drlee reads in a real Agent turn,
cloned the bound Slate repository, and ran three configuration tests. It did not
push, deploy, send external messages or provision a full business environment.
See [real results and limitations](../../docs/modules/external-access/index.html#real-smoke).

## Local-first Project Manager

Offline public-boundary tests use the compiled Rust CLI/server/SQLite/scheduler,
substituting only upstream Codex/OpenSandbox. They cover durable identity,
approvals, task queue fairness, scheduling, remote recovery, fail-closed transport,
single-daemon locking and non-destructive Skill installation:

```sh
cargo build --workspace
python3 -m unittest discover -s scripts/smoke -p test_project_manager.py -v
```

These tests also cover offline user-Home initialization, explicit auth import,
owned CODEX_HOME/CODEX_SQLITE_HOME binding and fail-closed missing Thread history.
To check real native transcript storage/resume locally without a model turn,
personal credential import or remote provisioning:

```sh
python3 scripts/smoke/test_codex_home_native.py -v
```

The native check uses an owned temporary Feynman Home, appends one harmless raw
history item via the experimental native API, and restarts/resumes the same
Thread. Codex lazily materializes the JSONL on history activity, not necessarily
on thread/start alone. Temporary state is removed after the check.

Opt-in real Spark + local Codex native Executor smoke:

```sh
python3 scripts/smoke/run_project_manager_smoke.py
```

Creates two project containers/volumes, exercises a real queued task, pause/resume,
automatic retained-volume recovery, daemon/thread restart and one real scheduled
task. Cleans its explicit test resources and temporary OpenSandbox service;
retains evidence and cached images/packages. It does not leave a durable remote
service running or modify Docker daemon settings. See
[current architecture and security boundary](../../docs/architecture/local-first-main-agent-v1.md).
This opt-in manager smoke now uses its Feynman-owned Home and an explicit private
auth-cache copy, removed at shutdown; it never links refresh writes to the source
profile. The older standalone native Executor smoke above has separate fixture
semantics and is not the product's user-Home initializer.
