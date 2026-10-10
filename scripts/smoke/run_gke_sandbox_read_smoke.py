"""Run a real Codex read smoke against a disposable GKE Agent Sandbox.

Requires an already-provisioned, disposable cluster with Agent Sandbox enabled.
This is NOT Feynman's production runtime or a cloud provisioning interface.
Cloud resource cleanup remains the operator's responsibility.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

NAMESPACE = "feynman-smoke"
HERE = Path(__file__).resolve().parent


def kubectl(config, *args, input_text=None, check=True, timeout=60):
    run = subprocess.run(
        ["kubectl", "--kubeconfig", str(config), "--request-timeout=45s", *args],
        input=input_text, text=True, capture_output=True, timeout=timeout,
    )
    if check and run.returncode:
        # Token creation/configuration output must never be included in errors.
        raise RuntimeError("kubectl step failed: " + " ".join(args[:3]))
    return run


def scoped_reader(admin, scratch, pod):
    role = {
        "apiVersion": "rbac.authorization.k8s.io/v1", "kind": "Role",
        "metadata": {"name": "assigned-pod-reader", "namespace": NAMESPACE},
        "rules": [
            {"apiGroups": [""], "resources": ["pods"], "resourceNames": [pod], "verbs": ["get"]},
            {"apiGroups": [""], "resources": ["pods/exec"], "resourceNames": [pod], "verbs": ["create", "get"]},
        ],
    }
    binding = {
        "apiVersion": "rbac.authorization.k8s.io/v1", "kind": "RoleBinding",
        "metadata": {"name": "assigned-pod-reader", "namespace": NAMESPACE},
        "subjects": [{"kind": "ServiceAccount", "name": "smoke-reader", "namespace": NAMESPACE}],
        "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": "assigned-pod-reader"},
    }
    kubectl(admin, "apply", "-f", "-", input_text=json.dumps({"apiVersion": "v1", "kind": "List", "items": [role, binding]}))
    admin_view = json.loads(kubectl(admin, "config", "view", "--minify", "--raw", "-o", "json").stdout)
    token = kubectl(admin, "-n", NAMESPACE, "create", "token", "smoke-reader", "--duration=1h").stdout.strip()
    config = {
        "apiVersion": "v1", "kind": "Config", "clusters": admin_view["clusters"],
        "contexts": [{"name": "smoke", "context": {"cluster": admin_view["clusters"][0]["name"],
                                                  "user": "smoke-reader", "namespace": NAMESPACE}}],
        "current-context": "smoke", "users": [{"name": "smoke-reader", "user": {"token": token}}],
    }
    path = scratch / "reader-kubeconfig.json"
    with open(path, "w", opener=lambda p, flags: os.open(p, flags, 0o600)) as stream:
        json.dump(config, stream)
    denied = kubectl(path, "-n", NAMESPACE, "auth", "can-i", "create", "secrets", check=False)
    if denied.stdout.strip() != "no":
        raise RuntimeError("Reader unexpectedly has secret write permission")
    other = kubectl(path, "-n", NAMESPACE, "get", "pod", "unassigned-pod", check=False)
    if other.returncode == 0 or "Forbidden" not in other.stderr:
        raise RuntimeError("Reader must be forbidden from accessing unassigned Pods")
    return path


def run(args):
    admin = Path(args.admin_kubeconfig).resolve()
    scratch = Path(args.scratch).resolve()
    scratch.mkdir(parents=True, exist_ok=True, mode=0o700)
    if (scratch / "agent-events.jsonl").exists():
        raise RuntimeError("Use a fresh scratch directory for each Agent run")
    crd_deadline = time.monotonic() + 600
    while True:
        crd = kubectl(admin, "get", "crd", "sandboxes.agents.x-k8s.io", "-o", "json", check=False)
        if crd.returncode == 0:
            conditions = json.loads(crd.stdout).get("status", {}).get("conditions", [])
            if any(c.get("type") == "Established" and c.get("status") == "True" for c in conditions):
                break
        if time.monotonic() >= crd_deadline:
            raise RuntimeError("Managed Agent Sandbox CRD was not established")
        print("Waiting for managed Agent Sandbox CRD", flush=True)
        time.sleep(15)
    namespace_probe = json.dumps({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": NAMESPACE}})
    while True:
        readiness = kubectl(admin, "apply", "--dry-run=server", "-f", "-", input_text=namespace_probe, check=False)
        if readiness.returncode == 0:
            break
        if "required-webhooks-limitation" not in readiness.stderr:
            raise RuntimeError("Namespace dry-run failed; inspect cluster authorization/admission")
        if time.monotonic() >= crd_deadline:
            raise RuntimeError("GKE Warden policy enforcement was not ready")
        print("Waiting for GKE Warden policy enforcement readiness", flush=True)
        time.sleep(15)
    print("Applying diagnostic Sandbox + PVC + deny-all network policy", flush=True)
    kubectl(admin, "apply", "-f", str(HERE / "gke_sandbox.yaml"))
    deadline = time.monotonic() + 900
    pod = None
    while time.monotonic() < deadline:
        pods = json.loads(kubectl(admin, "-n", NAMESPACE, "get", "pods", "-l", "feynman-purpose=smoke", "-o", "json").stdout)["items"]
        if len(pods) == 1:
            candidate = pods[0]
            ready = any(c.get("type") == "Ready" and c.get("status") == "True" for c in candidate.get("status", {}).get("conditions", []))
            if ready:
                pod = candidate["metadata"]["name"]
                if candidate["spec"]["runtimeClassName"] != "gvisor" or candidate["spec"].get("automountServiceAccountToken") is not False:
                    raise RuntimeError("Sandbox isolation prerequisites were not applied")
                break
        print("Waiting for sandbox node/image/PVC readiness", flush=True)
        time.sleep(15)
    if not pod:
        raise RuntimeError("Sandbox readiness timed out; inspect Pod events")
    print("Sandbox ready: " + pod, flush=True)
    seed = """
import json, pathlib, secrets, socket
root = pathlib.Path('/workspace')
probe = {'marker': secrets.token_hex(16), 'message': 'Hello from the remote GKE workspace', 'hostname': socket.gethostname()}
(root / 'probe.json').write_text(json.dumps(probe))
(root / 'note.txt').write_text('This file lives on the sandbox PVC, not the local Agent host.\\n')
assert not pathlib.Path('/var/run/secrets/kubernetes.io/serviceaccount/token').exists()
print(json.dumps({**probe, 'files': sorted(p.name for p in root.iterdir())}))
"""
    expected = json.loads(kubectl(admin, "-n", NAMESPACE, "exec", pod, "-c", "runtime", "--", "python3", "-c", seed).stdout)
    # The verifier alone holds expected. Never include it in the Agent prompt.
    reader = scratch / "reader-kubeconfig.json"
    try:
        scoped_reader(admin, scratch, pod)
        audit = scratch / "remote-audit.jsonl"
        workspace = scratch / "agent-workspace"
        workspace.mkdir(mode=0o700)
        schema = {"type": "object", "additionalProperties": False,
                  "properties": {"marker": {"type": "string"}, "message": {"type": "string"},
                                 "hostname": {"type": "string"}, "files": {"type": "array", "items": {"type": "string"}},
                                 "note": {"type": "string"}},
                  "required": ["marker", "message", "hostname", "files", "note"]}
        schema_path = scratch / "output-schema.json"
        schema_path.write_text(json.dumps(schema))
        agent_env = {"FEYNMAN_SMOKE_POD": pod, "FEYNMAN_SMOKE_NAMESPACE": NAMESPACE,
                     "FEYNMAN_SMOKE_KUBECONFIG": str(reader), "FEYNMAN_SMOKE_AUDIT": str(audit),
                     "PATH": os.environ["PATH"]}
        def quote(value):
            return json.dumps(str(value))
        env_toml = ", ".join(quote(key) + " = " + quote(value) for key, value in agent_env.items())
        mcp = ("mcp_servers = {sandbox_smoke = {command = " + quote(sys.executable)
               + ", args = [" + quote(HERE / "gke_sandbox_mcp.py") + "], env = {" + env_toml
               + "}, startup_timeout_sec = 30, tool_timeout_sec = 60}}")
        answer = scratch / "agent-answer.json"
        command = ["codex", "exec", "--ignore-user-config", "--ignore-rules", "--ephemeral", "--skip-git-repo-check",
                   "--sandbox", "read-only", "--json", "--color", "never", "--cd", str(workspace),
                   "--output-schema", str(schema_path), "--output-last-message", str(answer),
                   "-c", "features.shell_tool=false", "-c", "features.unified_exec=false",
                   "-c", "features.plugins=false", "-c", "features.apps=false", "-c", "features.hooks=false",
                   "-c", "features.multi_agent=false", "-c", "features.browser_use=false",
                   "-c", "features.computer_use=false", "-c", "features.image_generation=false",
                   "-c", "features.in_app_browser=false",
                   "-c", "features.skill_search=false",
                   "-c", "features.skip_host_skill_discovery=true", "-c", 'web_search="disabled"', "-c", mcp,
                   "Inspect the assigned REMOTE sandbox using sandbox_list_files and sandbox_read_file. "
                   "Read both probe.json and note.txt. Return the observed marker, message, hostname, file names, "
                   "and exact note text in the requested JSON schema. Do not guess values. Do not use host tools."]
        print("Launching fresh Codex Agent; only diagnostic MCP is configured, local command tools disabled", flush=True)
        with (scratch / "agent-events.jsonl").open("w") as output, (scratch / "agent-stderr.log").open("w") as errors:
            agent = subprocess.run(command, stdout=output, stderr=errors, timeout=300)
        if agent.returncode:
            raise RuntimeError("Codex smoke failed; inspect local diagnostic logs")
        observed = json.loads(answer.read_text())
        calls = [json.loads(line) for line in audit.read_text().splitlines()] if audit.exists() else []
        required_calls = {("list", None), ("read", "probe.json"), ("read", "note.txt")}
        assert required_calls.issubset({(c["operation"], c["path"]) for c in calls}), "Agent did not perform all remote reads"
        assert all(observed[key] == expected[key] for key in ("marker", "message", "hostname")), "Agent response differs from remote probe"
        assert observed["files"] == expected["files"], "Incorrect remote directory listing"
        assert observed["note"] == "This file lives on the sandbox PVC, not the local Agent host.\n", "Incorrect note content"
        events = [json.loads(line) for line in (scratch / "agent-events.jsonl").read_text().splitlines() if line.startswith("{")]
        command_items = [e for e in events if e.get("item", {}).get("type") == "command_execution"]
        assert not command_items, "Agent unexpectedly used host command tools"
        mcp_items = [e["item"] for e in events if e.get("type") == "item.completed"
                     and e.get("item", {}).get("type") == "mcp_tool_call"]
        assert mcp_items and all(i.get("server") == "sandbox_smoke" for i in mcp_items), "Missing remote MCP call evidence or unexpected MCP server"
        summary = {"passed": True, "pod": pod, "runtimeClass": "gvisor", "service_account_token_mounted": False,
                   "reader_secret_writes_denied": True, "reader_unassigned_pod_denied": True,
                   "remote_calls": calls, "marker_sha256": hashlib.sha256(expected["marker"].encode()).hexdigest(),
                   "marker_matches": True, "host_command_count": len(command_items),
                   "agent_mcp_tools": [i.get("tool") for i in mcp_items],
                   "files": observed["files"], "agent_runs_in": "local host, not GKE"}
        (scratch / "verification.json").write_text(json.dumps(summary, indent=2) + "\n")
        print(json.dumps(summary, indent=2), flush=True)
    finally:
        reader.unlink(missing_ok=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--admin-kubeconfig", required=True)
    parser.add_argument("--scratch", required=True)
    options = parser.parse_args()
    try:
        run(options)
    except (RuntimeError, AssertionError, subprocess.TimeoutExpired) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
