---
name: feynman
description: "Manage Feynman projects from a local harness: inspect Main Agents, create isolated Workspaces, delegate project tasks, continue conversations, configure external tools, or schedule project work."
---

# Feynman project manager

The current harness is the user's manager, not the Project's Main Agent. Each
Project has its own persistent Main Agent conversation, work queue and Workspace.
Reviewer is optional and disabled in this version. Use the installed `feynman`
CLI; inspect subcommand `--help` when arguments are unclear.

1. Discover: run `feynman --json health` then `feynman --json status`. Select an
   existing Project by its exact ID. If multiple Projects match, resolve the
   target with the user before sending work.
2. Inspect: read `status PROJECT_ID`, `conversation PROJECT_ID`, `tasks list
   PROJECT_ID`, and `workspace show PROJECT_ID` as relevant. Recorded Workspace
   state is a cache; `workspace refresh PROJECT_ID` checks the remote provider.
3. Act within the user's request:
   - Create a Project with `projects create --name NAME`, then explicitly create
     its Workspace with `workspace create PROJECT_ID --provider opensandbox` or
     `--provider local`. A remote Project requires server-side operator setup.
   - Delegate implementation as `tasks create PROJECT_ID --title TITLE --prompt
     PROMPT`; include constraints and acceptance checks. A queued task executes
     automatically while the server runs. Sending a message with `send PROJECT_ID
     --message TEXT` starts a turn immediately, and conflicts if the Agent is busy.
   - Read task `show`, then `runs PROJECT_ID RUN_ID --wait --timeout-seconds 60`
     for bounded progress. Retain returned IDs; resume inspection after timeout
     instead of submitting duplicate work. Inspect conversation for the response.
4. Report the Project/task/run IDs, observed state, results and any pending human
   decision. `completed` means the Agent turn ended; verify acceptance evidence
   before declaring the application correct or deployed.

## Scheduled work

Use `schedules create PROJECT_ID --title TITLE --prompt PROMPT --every-seconds N`
or `--at RFC3339_WITH_OFFSET`; this creates a paused plan. Add `--enable` or use
`schedules resume` only when the user has authorized that recurring/one-shot work.
The local server must be running and awake. Missed occurrences coalesce; one
Main Agent works at a time. `schedules pause` prevents future occurrences, but
already queued tasks require separate `tasks cancel`. Heartbeat is liveness,
not a scheduled task or proof of work.

## Boundaries and recovery

- Project creation, task submission and schedule enablement are mutations; a
  request for status alone authorizes inspection only. Keep external-tool grants
  and native approvals under explicit user authority. Use `approve` only for a
  specific pending request and decision; otherwise report it.
- Inspect `external-tools list PROJECT_ID`. DWS device login requires a
  `dws.auth.device` grant. `dws-auth request PROJECT_ID --reason REASON` returns a
  verification link/code; present them to the user and poll `dws-auth status`.
  Authentication does not grant business tools. Keep credentials outside the
  Skill and Workspace.
- A stopped Workspace needs explicit `workspace resume`; queued tasks wait while
  it is paused. Authorized work restores an expired/missing container from its
  retained volume; `workspace restore` can also do this explicitly. Restoration
  retains files, not rootfs/process memory. Pause is not resource-free. A remote failure is not
  permission to execute the Project's commands in the user's local directory.
- Failed/interrupted tasks are retained for inspection and are not auto-replayed.
  Check the persisted Run and clarify uncertain side effects before a new task.

Use `--json` for non-interactive operations. Interactive human chat remains
available as `feynman chat PROJECT_ID`; JSON mode never answers approvals itself.

## Native CLI access and repository development

For external access, inspect `access providers` and `access status PROJECT_ID`.
The Project Agent uses MCP `feynman_access` to request one capability; the manager
can use `access request` for the same persisted request. A request is complete
when its ID and owner action are reported, not when the account appears logged in.

Under the user's authorization, the owner manager can register an existing native
login with `access connections import-gitlab --hostname HOST`, `import-slate
--hostname HOST --profile PROFILE`, or `import-drlee --hostname HOST`. Fresh
Slate/Drlee login uses `login-slate` / `login-drlee --hostname HOST`; present the
native browser flow to the user. Slate uses PKCE, not device codes. Only public
Connection metadata is returned; keep private helper stdout and credentials out
of chat, files in the Workspace and command arguments.

After choosing the Connection, bind the exact Project/provider and grant its
specific business tools. Git additionally needs the user's exact
`namespace/repository`; Drlee message reads need an owner-bound channel UUID via
`access bind ... --resource-id UUID`. Existing authentication never supplies
these permissions automatically. Check the live call result before reporting
success; configured binding metadata is not verification.

For authorized repository initialization, create/restore a running OpenSandbox,
bind provider `git` to the GitLab Connection and selected repository, then grant
`git.workspace.clone`. The Agent's `git_workspace_clone` MCP accepts only a UUID
`idempotency_key` and targets `/workspace/repo`. Retain that key after timeout:
inspect the existing Run and Workspace before retrying the same operation.
Existing repository files and failed partial clones are retained. Once cloned,
the Agent develops/tests with native Sandbox tools and follows the repository's
instructions. This grant permits clone and local development, not push, MR
writes, deployment, submodules or unattended installation scripts. Observational
Slate/Drlee tools use the central native CLI bridge, not container CLI logins.
