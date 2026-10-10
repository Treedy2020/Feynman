# DWS Device Authorization — Local-first V1

Implemented 2026-09-29. The Agent can request device authentication through the
project MCP; Feynman owns an asynchronous DWS CLI process, presents the public
challenge to a CLI or future frontend, and independently verifies the completed
login. This is authentication orchestration, **not** sandbox credential delivery.

## Policy and ownership

- A project owner explicitly grants `dws.auth.device` before the Agent can start
  authentication. OAuth consent is completed by the user in DingTalk's browser UI.
- The only accepted input is a short reason. The Agent cannot supply an executable,
  endpoint, token, arbitrary flags, output directory, or another project's context.
- `feynman_dws_authorization` supports `request`, `list`, `status`, `cancel`, and
  `check`. Its MCP annotation is not read-only; OAuth login changes account state.
- The MCP process forwards calls to the central loopback server. Closing the MCP
  process or ending an Agent turn does not destroy an authorization job.
- OAuth completion does not automatically grant `dws.aisearch.enterprise` or any
  other business operation. No `--recommend`, `--yes`, or app-secret override is
  added by this adapter.
- The current server remains local development only: fixed local identity,
  loopback binding, no remote-user authentication. MCP context is supplied by the
  server, not model-selected request parameters.

## Flow and frontend contract

```text
Agent / CLI: request
    -> project grant check
    -> preparing (return immediately)
    -> waiting_for_user (URL + user_code + deadline)
    -> user consents in DingTalk
    -> DWS CLI finishes and persists credentials privately
    -> fresh dws auth status verifies token_valid + account identity
    -> authorized
```

Other outcomes are `expired`, `denied`, `failed`, `cancelled`, `interrupted`, and
`authentication_required`. A transient check error returns an error rather than
pretending a new successful verification occurred. Account changes are rejected.

| HTTP operation | Meaning |
| --- | --- |
| `POST /v1/projects/{project}/dws-authorizations` | `{ "reason": "Read project facts" }`; returns 202 and an authorization record |
| `GET /v1/projects/{project}/dws-authorizations` | List the current user's project requests |
| `GET /v1/projects/{project}/dws-authorizations/{id}` | Read progress and public challenge |
| `DELETE /v1/projects/{project}/dws-authorizations/{id}` | Cancel an unfinished request |
| `POST /v1/projects/{project}/dws-authorizations/{id}/check` | Send `{}` as JSON; fresh check, DWS refreshes when required |

Frontend clients poll the GET operation, for example once per second. During
`waiting_for_user`, render `verification_uri_complete` (or `verification_uri`) as
a link, `user_code` as the user-facing code, and `expires_at` as the challenge
deadline. Only verified HTTPS DingTalk domains are accepted; the private
`device_code`, OAuth authorization code, and token values are never API fields.
Responses use `Cache-Control: no-store` and `Referrer-Policy: no-referrer`.

`checked_at` records an actual completed authentication check, not a Session
Heartbeat. The DWS process handles device polling itself; frontend polling does
not start another DWS login or refresh loop. After success, GET is historical
state; use POST `/check` for a fresh validity check. There is no perpetual refresh
daemon or automatic Agent wake-up in this slice.

Terminal chat watches pending authorizations even while waiting for user input,
then displays transitions. Standalone CLI usage:

```sh
feynman external-tools grant PROJECT_ID dws.auth.device
feynman dws-auth request PROJECT_ID --reason "Read project facts" --wait
feynman dws-auth list PROJECT_ID
feynman dws-auth status PROJECT_ID AUTHORIZATION_ID --wait
feynman dws-auth check PROJECT_ID AUTHORIZATION_ID
feynman dws-auth cancel PROJECT_ID AUTHORIZATION_ID
```

## Persistence, process lifecycle, and credentials

SQLite stores typed public metadata, scoped by tenant, project, and requesting
user. A unique pending-request index and a service lock deduplicate repeated
requests. Compare-and-swap updates prevent completion overwriting cancellation.
There are at most 16 active jobs. Checks are serialized to avoid concurrent
refresh-token rotation in this local MVP.

Credentials stay in `.feynman/private/dws/<authorization-id>/`, outside Agent
workspaces. Each request has independent `DWS_CONFIG_DIR` and `DWS_KEYCHAIN_DIR`.
The adapter clears inherited DWS settings and app secrets; file-DEK mode
(`DWS_DISABLE_KEYCHAIN=1`) is deliberate for this private development store.
Directories are mode 0700. No personal DWS profile or system Keychain is migrated
or changed. This is not a production credential vault or a short-lived lease.

The owned child uses `kill_on_drop`; cancellation and graceful shutdown abort
its task. Pending grants are rechecked during execution. A 600-second maximum
wait also bounds provider polling. On startup, unfinished records become
`interrupted` with their challenge cleared, rather than claiming recovery of the
OAuth process. After an abrupt OS crash/SIGKILL, this slice does not promise an
instantaneous child-process kill; the CLI has its own bounded device deadline.

DWS currently writes a human-readable device box to stderr and final JSON to
stdout. The adapter recognizes the observed box with a fixed language, strips
ANSI, and allowlists only its link/code/expiry fields. It bounds both streams,
never logs raw output, and reports fixed error codes. CLI output changes may
require an adapter update; a future structured DWS progress contract is preferable.

Cancelling a request or revoking its Feynman grant **does not revoke an already
issued provider token**. Completed credentials are retained privately; provider
revocation and remote runtime mounting are separate future operations. As of the
External Access V1 update, an owner can register the freshly verified request as a
Connection, bind a project, and separately grant enterprise search. The search
path uses that cache, rechecks identity and never falls back to the host profile.
See [DWS submodule](../modules/external-access/dws.html). Success responses report
`credential_location: local-control-plane`, `sandbox_installed: false`.

## Verification

`test_dws_authorization.py` exercises real compiled Rust HTTP, MCP, and CLI paths;
only the external DWS process is substituted with an offline fixture. Coverage:
grant denial, malformed inputs, URLs/code presentation, deduplication, identity
verification, no implicit business grant, project isolation, cancellation/process
termination, grant revocation, expiry, refusal, untrusted URLs, malformed output,
secret canaries, fresh checks, account changes, and restart persistence.

```sh
cargo build --workspace
python3 -m unittest discover -s scripts/smoke -p test_dws_authorization.py -v
```

A real device request can be tested separately. It deliberately waits for user
consent and cancels unfinished work at the deadline; it uses isolated storage:

```sh
python3 scripts/smoke/run_dws_device_smoke.py --dws /absolute/path/to/dws --wait-seconds 90
```

The real smoke must distinguish “link retrieved” from “user consent and verified
authentication completed”; the offline successful fixture is not proof of real
provider login.

On 2026-09-29 the real MCP → Rust server → installed DWS smoke reached
`waiting_for_user` with the actual `https://login.dingtalk.com/oauth2/device/verify.htm`
verification URI and a user code. No browser consent was completed during the
90-second test window; the request became `cancelled`, and the owned server was
stopped. Real successful login remains unverified. Public evidence:
`/var/folders/tb/bxhyq9ld1gg6lpb97zk98m8c0000gn/T/feynman-dws-device-smoke-inkyewlh/`.
