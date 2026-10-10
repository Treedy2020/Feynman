#!/usr/bin/env python3
"""Real Rust CLI -> server -> OpenSandbox -> Docker file read smoke."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import socket
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
TOOL = "sandbox.files.smoke"


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def request(base, path, key=None, method="GET", body=None):
    headers = {"content-type": "application/json"}
    if key:
        headers["OPEN-SANDBOX-API-KEY"] = key
    req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), headers=headers, method=method)
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req, timeout=30) as response:
        raw = response.read()
        return json.loads(raw) if raw else None


def wait_for(fn, timeout=120):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            result = fn()
            if result:
                return result
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.5)
    raise RuntimeError("fixture readiness timed out")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ssh", default="simonlong@spark-d304.tail242882.ts.net")
    parser.add_argument("--remote-runtime-dir", required=True, help="isolated directory containing venv with opensandbox-server==0.2.3")
    parser.add_argument("--remote-port", type=int, default=18080)
    parser.add_argument("--scratch", type=Path, required=True, help="new empty local directory; logs and verification only")
    parser.add_argument("--agent", action="store_true", help="also run a real Feynman Main Agent turn using existing local Codex authentication")
    args = parser.parse_args()
    scratch = args.scratch.resolve()
    scratch.mkdir(mode=0o700, parents=True, exist_ok=True)
    if list(scratch.iterdir()):
        raise RuntimeError("scratch must be empty")
    os.chmod(scratch, 0o700)
    ssh = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "StrictHostKeyChecking=yes", args.ssh]

    def remote(command, payload=None):
        result = subprocess.run(ssh + [command], input=None if payload is None else json.dumps(payload), text=True, capture_output=True, timeout=60)
        if result.returncode:
            raise RuntimeError("remote fixture command failed: " + result.stderr[-1000:])
        return result.stdout.strip()

    run_dir = remote("mktemp -d /tmp/feynman-os-run-XXXXXXXX")
    helper = run_dir + "/helper.py"
    subprocess.run(["scp", "-q", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", str(Path(__file__).with_name("opensandbox_remote.py")), args.ssh + ":" + helper], check=True)
    remote_python = args.remote_runtime_dir.rstrip("/") + "/venv/bin/python"

    def operator(action, payload=None):
        command = shlex.join([remote_python, helper, action, "--run-dir", run_dir, "--port", str(args.remote_port)])
        return json.loads(remote(command, payload))

    fixture = None
    tunnel = None
    server = None
    config_path = scratch / "connection.json"
    sandbox_ids = []
    verification = {"passed": False, "remote_run_dir": run_dir}
    base = None
    started = time.monotonic()
    try:
        fixture = operator("start")
        port = free_port()
        base = f"http://127.0.0.1:{port}"
        tunnel = subprocess.Popen(ssh[:-1] + ["-N", "-o", "ExitOnForwardFailure=yes", "-L", f"127.0.0.1:{port}:127.0.0.1:{args.remote_port}", args.ssh], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        wait_for(lambda: request(base, "/v1/sandboxes", fixture["api_key"]))
        create_body = {
            "image": {"uri": "python:3.12-slim"},
            "entrypoint": ["python3", "-c", "import time; time.sleep(1800)"],
            "resourceLimits": {"cpu": "500m", "memory": "512Mi"},
            "timeout": 900,
            "metadata": {"feynman-smoke": fixture["run_id"]},
            "volumes": [{"name": "workspace", "pvc": {"claimName": fixture["volume"], "createIfNotExists": False, "deleteOnSandboxTermination": False}, "mountPath": "/workspace"}],
        }

        def create_sandbox():
            begin = time.monotonic()
            result = request(base, "/v1/sandboxes", fixture["api_key"], "POST", create_body)
            sandbox_id = result["id"]
            sandbox_ids.append(sandbox_id)
            def ready():
                current = request(base, f"/v1/sandboxes/{sandbox_id}", fixture["api_key"])
                state = current["status"]["state"]
                if state in ["Failed", "Terminated"]:
                    raise RuntimeError("sandbox entered " + state)
                if state != "Running":
                    return False
                request(base, f"/v1/sandboxes/{sandbox_id}/proxy/44772/ping", fixture["api_key"])
                return True
            wait_for(ready, timeout=180)
            return sandbox_id, round(time.monotonic() - begin, 2)

        sandbox_id, create_seconds = create_sandbox()
        marker = secrets.token_hex(24)
        expected = {"probe.json": json.dumps({"marker": marker, "source": "spark-opensandbox-docker"}), "note.txt": "Feynman reads this file through OpenSandbox.\n"}
        independent = operator("seed", {"sandbox_id": sandbox_id, "files": expected})
        assert independent["files"] == expected
        verification.update({"sandbox_id": sandbox_id, "container_id": independent["container_id"], "loopback_only": True, "create_seconds": create_seconds})
        print("Remote sandbox ready; loopback-only ports verified.", flush=True)

        server_port = free_port()
        server_base = f"http://127.0.0.1:{server_port}"
        env = {**os.environ, "FEYNMAN_BIND": f"127.0.0.1:{server_port}", "FEYNMAN_DATA_DIR": str(scratch / "data"),
               "FEYNMAN_DB": str(scratch / "data/rust-platform.sqlite3"), "FEYNMAN_WORKSPACE_ROOT": str(scratch / "workspaces"),
               "FEYNMAN_OPENSANDBOX_SMOKE_CONFIG": str(config_path)}
        server_log = open(scratch / "feynman.log", "ab")

        def start_server():
            proc = subprocess.Popen([str(ROOT / "target/debug/feynman-server")], env=env, stdout=server_log, stderr=server_log)
            def healthy():
                if proc.poll() is not None:
                    raise RuntimeError("Feynman test server exited; see feynman.log")
                return request(server_base, "/v1/health")
            try:
                wait_for(healthy, timeout=20)
            except BaseException:
                proc.terminate()
                proc.wait(timeout=15)
                raise
            return proc

        # Boot once without adapter to obtain a real persisted Project via CLI.
        env.pop("FEYNMAN_OPENSANDBOX_SMOKE_CONFIG")
        server = start_server()

        def cli(*words, error=None):
            result = subprocess.run([str(ROOT / "target/debug/feynman"), "--server", server_base, *words], capture_output=True, text=True, timeout=30)
            if error:
                assert result.returncode != 0 and error in result.stderr, "CLI did not deny as expected"
                return None
            if result.returncode:
                raise RuntimeError("Feynman CLI failed: " + result.stderr[-1500:])
            return result.stdout

        output = cli("projects", "create", "--name", "OpenSandbox smoke")
        project_id = re.search(r"\(([^()\s]+)\)\s*$", output).group(1)
        server.terminate()
        server.wait(timeout=15)
        connection = {"base_url": base, "api_key": fixture["api_key"], "tenant_id": "local", "project_id": project_id, "sandbox_id": sandbox_id}

        def save_connection():
            with open(config_path, "w", opener=lambda p, f: os.open(p, f, 0o600)) as out:
                json.dump(connection, out)

        save_connection()
        env["FEYNMAN_OPENSANDBOX_SMOKE_CONFIG"] = str(config_path)
        server = start_server()
        assert TOOL in cli("external-tools", "list", project_id), "OpenSandbox smoke tool is not registered"
        cli("external-tools", "invoke", project_id, TOOL, "--arguments", '{"operation":"list"}', error="TOOL_NOT_GRANTED")
        cli("external-tools", "grant", project_id, TOOL)

        def invoke(arguments):
            return json.loads(cli("external-tools", "invoke", project_id, TOOL, "--arguments", json.dumps(arguments)))

        listing = invoke({"operation": "list"})
        assert sorted(listing["data"]["files"]) == ["note.txt", "probe.json"]
        for name, content in expected.items():
            result = invoke({"operation": "read", "file": name})
            assert result["source"] == TOOL and result["data"]["sandbox_id"] == sandbox_id
            assert result["data"]["content"] == content, "Feynman read differs from independently seeded file"
        cli("external-tools", "invoke", project_id, TOOL, "--arguments", '{"operation":"read","file":"../../etc/passwd"}', error="INVALID_INPUT")
        other = re.search(r"\(([^()\s]+)\)\s*$", cli("projects", "create", "--name", "Unbound smoke project")).group(1)
        cli("external-tools", "grant", other, TOOL)
        cli("external-tools", "invoke", other, TOOL, "--arguments", '{"operation":"list"}', error="NOT_FOUND")
        cli("external-tools", "revoke", project_id, TOOL)
        cli("external-tools", "invoke", project_id, TOOL, "--arguments", '{"operation":"list"}', error="TOOL_NOT_GRANTED")
        cli("external-tools", "grant", project_id, TOOL)
        verification["cli_reads_and_denials"] = True
        print("Rust CLI reads match independent fixture; grant, revoke, path and project checks passed.", flush=True)

        mcp_env = {**env, "FEYNMAN_DB": str(scratch / "data/rust-platform.sqlite3"), "FEYNMAN_AGENT_USER_ID": "local-user", "FEYNMAN_AGENT_TENANT_ID": "local", "FEYNMAN_AGENT_PROJECT_ID": project_id}
        messages = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "1"}}},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "sandbox_smoke_files", "arguments": {"operation": "read", "file": "probe.json"}}},
        ]
        mcp = subprocess.run([str(ROOT / "target/debug/feynman-server"), "external-tools-mcp"], env=mcp_env,
                             input="".join(json.dumps(m) + "\n" for m in messages), text=True, capture_output=True, timeout=30, check=True)
        replies = {r["id"]: r for r in map(json.loads, mcp.stdout.splitlines())}
        assert any(t["name"] == "sandbox_smoke_files" for t in replies[2]["result"]["tools"])
        assert replies[3]["result"]["isError"] is False
        assert json.loads(replies[3]["result"]["content"][0]["text"])["data"]["content"] == expected["probe.json"]
        verification["mcp_read"] = True

        if args.agent:
            prompt = ('请只使用本项目 Feynman MCP 的 sandbox_smoke_files 工具：先 list，再 read probe.json 和 note.txt。'
                      '不要使用 shell 或其他外部工具，不要猜内容。最终只返回 JSON：'
                      '{"files":[文件名列表],"marker":"probe.json 中 marker 的实际值","note":"note.txt 原文"}。')
            run = request(server_base, f"/v1/projects/{project_id}/messages", method="POST", body={"content": prompt})["run"]
            def agent_finished():
                feed = request(server_base, f"/v1/projects/{project_id}/runs/{run['id']}/events")
                status = feed["run"]["status"]
                if status in ["failed", "interrupted", "waiting_approval"]:
                    raise RuntimeError("Agent smoke did not complete: " + status)
                return feed if status == "completed" else False
            feed = wait_for(agent_finished, timeout=180)
            snapshot = request(server_base, f"/v1/projects/{project_id}/conversation")
            answer = next(m["content"] for m in snapshot["messages"] if m["id"] == run["assistant_message_id"])
            (scratch / "agent-answer.txt").write_text(answer)
            (scratch / "agent-events.json").write_text(json.dumps(feed["events"], indent=2))
            match = re.search(r"\{[\s\S]*\}", answer)
            assert match, "Agent did not return JSON"
            value = json.loads(match.group(0))
            assert sorted(value["files"]) == ["note.txt", "probe.json"]
            assert value["marker"] == marker and value["note"] == expected["note.txt"], "Agent answer differs from independent fixture"
            activities = [e["payload"] for e in feed["events"] if e["kind"] == "tool_activity"]
            assert any("sandbox_smoke_files" in a.get("tool", "") for a in activities), "no native sandbox tool activity"
            assert not any(a.get("tool") == "Shell command" for a in activities), "Agent used shell instead of file tool"
            verification["agent_read"] = True
            print("Main Agent read the unseen random marker through Feynman MCP.", flush=True)

        # Destroy the container, reattach the same named volume, and read again.
        request(base, f"/v1/sandboxes/{sandbox_id}", fixture["api_key"], "DELETE")
        sandbox_ids.remove(sandbox_id)
        sandbox_id, recreate_seconds = create_sandbox()
        independent = operator("inspect", {"sandbox_id": sandbox_id})
        assert independent["files"] == expected, "workspace files did not survive container replacement"
        server.terminate()
        server.wait(timeout=15)
        connection["sandbox_id"] = sandbox_id
        save_connection()
        server = start_server()
        assert invoke({"operation": "read", "file": "probe.json"})["data"]["content"] == expected["probe.json"]
        verification.update({"persistent_volume_recovery": True, "recreate_seconds": recreate_seconds, "marker_sha256": hashlib.sha256(marker.encode()).hexdigest(), "passed": True})
        print("Container replacement preserved workspace; Feynman read passed again.", flush=True)
    finally:
        try:
            if server:
                server.terminate()
                server.wait(timeout=15)
            if fixture:
                cleanup = operator("cleanup")
                verification["cleanup"] = cleanup
                assert cleanup["remaining_containers"] == 0 and cleanup["remaining_volumes"] == 0
        except BaseException:
            verification["passed"] = False
            raise
        finally:
            config_path.unlink(missing_ok=True)
            if tunnel:
                tunnel.terminate()
                tunnel.wait(timeout=15)
            verification["elapsed_seconds"] = round(time.monotonic() - started, 2)
            (scratch / "verification.json").write_text(json.dumps(verification, indent=2) + "\n")
    print(json.dumps(verification, indent=2))


if __name__ == "__main__":
    main()
