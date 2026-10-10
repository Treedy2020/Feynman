#!/usr/bin/env python3
"""Opt-in real DWS device request through the Rust MCP bridge; only a user can consent.

Uses isolated Feynman/DWS storage, never the personal profile. Cancels a still
pending flow and stops owned processes at the deadline. Public evidence is retained.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

from test_dws_authorization import ROOT, call, grant, path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dws", required=True)
    parser.add_argument("--wait-seconds", type=int, default=60)
    args = parser.parse_args()
    if not 1 <= args.wait_seconds <= 600:
        parser.error("wait-seconds must be between 1 and 600")
    executable = Path(args.dws).resolve(strict=True)
    evidence = Path(tempfile.mkdtemp(prefix="feynman-dws-device-smoke-"))
    evidence.chmod(0o700)
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    env = dict(os.environ, FEYNMAN_BIND=f"127.0.0.1:{port}",
               FEYNMAN_DATA_DIR=str(evidence / "data"), FEYNMAN_DWS_BIN=str(executable))
    env.pop("FEYNMAN_DB", None)
    env.pop("FEYNMAN_WORKSPACE_ROOT", None)
    record = None
    project = None
    with (evidence / "server.log").open("w") as logs:
        server = subprocess.Popen([str(ROOT / "target/debug/feynman-server")], cwd=ROOT,
                                  env=env, stdout=logs, stderr=logs)
        try:
            until = time.monotonic() + 10
            while True:
                try:
                    call(base, "GET", "/v1/health")
                    break
                except OSError:
                    if server.poll() is not None or time.monotonic() >= until:
                        raise RuntimeError("Feynman server failed to start")
                    time.sleep(0.1)
            project = call(base, "POST", "/v1/projects", {"name": "DWS device smoke"})[1]["project"]["id"]
            grant(base, project)
            mcp_env = dict(env, FEYNMAN_DB=str(evidence / "data/rust-platform.sqlite3"),
                           FEYNMAN_ACCESS_SERVER_URL=base, FEYNMAN_AGENT_PROJECT_ID=project,
                           FEYNMAN_AGENT_TENANT_ID="local", FEYNMAN_AGENT_USER_ID="local-user")
            messages = [
                {"jsonrpc":"2.0", "id":1,"method":"initialize"},
                {"jsonrpc":"2.0", "id":2,"method":"tools/call","params":{
                    "name":"feynman_dws_authorization", "arguments":{"operation":"request", "reason":"Validate device authorization lifecycle"}}},
            ]
            result = subprocess.run([str(ROOT / "target/debug/feynman-server"), "external-tools-mcp"],
                                    input="\n".join(json.dumps(item) for item in messages) + "\n",
                                    env=mcp_env, capture_output=True, text=True, timeout=10)
            if result.returncode != 0:
                raise RuntimeError("MCP bridge failed")
            response = json.loads(result.stdout.splitlines()[-1])["result"]
            if response.get("isError"):
                raise RuntimeError("authorization tool denied the smoke request")
            record = json.loads(response["content"][0]["text"])["data"]["authorization"]
            print(json.dumps({"evidence_dir":str(evidence),"server":base,"project_id":project,"authorization_id":record["id"]}), flush=True)
            until = time.monotonic() + args.wait_seconds
            last = None
            while time.monotonic() < until:
                _, value = call(base, "GET", path(project, record["id"]))
                record = value["authorization"]
                summary = {"status":record["status"], "verification_uri":record["verification_uri"],
                           "verification_uri_complete":record["verification_uri_complete"], "user_code":record["user_code"]}
                if summary != last:
                    print(json.dumps(summary, ensure_ascii=False), flush=True)
                    last = summary
                if record["status"] not in ("preparing", "waiting_for_user"):
                    break
                time.sleep(1)
        finally:
            try:
                if record and record["status"] in ("preparing", "waiting_for_user"):
                    record = call(base, "DELETE", path(project, record["id"]))[1]["authorization"]
                if record:
                    (evidence / "authorization.json").write_text(json.dumps(record, ensure_ascii=False, indent=2))
            finally:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)
    print(json.dumps({"final_status":record["status"] if record else None, "owned_server_stopped":True,"evidence_dir":str(evidence)}), flush=True)


if __name__ == "__main__":
    main()
