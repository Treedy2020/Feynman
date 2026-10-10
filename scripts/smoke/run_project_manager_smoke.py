#!/usr/bin/env python3
"""Opt-in: real Rust CLI + server + local Codex + Spark OpenSandbox native executor.

Deletes only the explicit test Projects' containers/volumes and the service fixture.
Keeps image/package caches and local evidence; never changes Docker daemon settings.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import shlex
import socket
import subprocess
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).parent
OPENER = build_opener(ProxyHandler({}))


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def request(base, path, method="GET", body=None):
    req = Request(base + path, method=method, data=None if body is None else json.dumps(body).encode(), headers={} if body is None else {"Content-Type": "application/json"})
    try:
        response = OPENER.open(req, timeout=45)
    except HTTPError as error:
        response = error
    with response:
        value = response.read()
        result = json.loads(value) if value else {}
        if response.status >= 400:
            raise RuntimeError(f"{method} {path} failed: {result}")
        return result


def wait_for(fn, predicate=bool, seconds=60):
    until = time.monotonic() + seconds
    while True:
        try:
            value = fn()
            if predicate(value):
                return value
        except OSError:
            pass
        if time.monotonic() >= until:
            raise RuntimeError("smoke readiness timed out")
        time.sleep(0.2)


def private_json(path, value):
    with open(path, "x", opener=lambda p, f: os.open(p, f, 0o600)) as stream:
        json.dump(value, stream)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--ssh", default="simonlong@spark-d304.tail242882.ts.net")
    parser.add_argument("--remote-runtime-dir", default="/tmp/feynman-opensandbox-smoke.tGx9qM")
    parser.add_argument("--remote-package-dir", default="/tmp/feynman-native-executor-LobzkDCF/package")
    parser.add_argument("--remote-port", type=int, default=18080)
    parser.add_argument("--codex", type=Path, default=Path("/opt/homebrew/Caskroom/codex/0.157.1/bin/codex"))
    args = parser.parse_args()
    scratch = Path(tempfile.mkdtemp(prefix="feynman-project-manager-smoke-"))
    print("Evidence: " + str(scratch), flush=True)
    ssh = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", args.ssh]

    def remote(command, body=None):
        result = subprocess.run(ssh + [command], text=True, input=None if body is None else json.dumps(body), capture_output=True, timeout=180)
        if result.returncode:
            raise RuntimeError("trusted operator command failed: " + result.stderr[-2000:])
        return result.stdout.strip()

    run_dir = remote("mktemp -d /tmp/feynman-os-run-XXXXXXXX")
    files = [(HERE / "opensandbox_remote.py", "opensandbox_remote.py"), (HERE / "project_manager_remote.py", "project_manager_remote.py"),
             (ROOT / "scripts/opensandbox/build_executor_image.py", "build_executor_image.py"), (ROOT / "infra/opensandbox/Dockerfile", "Dockerfile")]
    for path, name in files:
        subprocess.run(["scp", "-q", "-o", "BatchMode=yes", str(path), args.ssh + ":" + run_dir + "/" + name], check=True)
    python = args.remote_runtime_dir + "/venv/bin/python"

    def operator(action):
        return json.loads(remote(shlex.join([python, run_dir + "/opensandbox_remote.py", action, "--run-dir", run_dir, "--port", str(args.remote_port)])))

    def resources(action, projects):
        return json.loads(remote(shlex.join([python, run_dir + "/project_manager_remote.py", action]), {"projects": projects}))

    source_home = Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex-personal")))
    data = scratch / "data"
    data.mkdir(mode=0o700)
    home = data / "codex"
    home.mkdir(mode=0o700)
    # This opt-in smoke explicitly copies its auth cache, never links refresh
    # writes to the user's harness. The private test copy is removed in finally.
    with open(home / "auth.json", "xb", opener=lambda p, f: os.open(p, f, 0o600)) as stream:
        stream.write((source_home / "auth.json").read_bytes())
    # Own temporary profile: only Feynman's project MCP is added by the product runtime.
    (home / "config.toml").write_text('model = "gpt-6-sol"\nmodel_reasoning_effort = "low"\nweb_search = "disabled"\n[features]\nplugins = false\napps = false\nhooks = false\nbrowser_use = false\ncomputer_use = false\nimage_generation = false\nskill_search = false\nskip_host_skill_discovery = true\n')
    fixture = None
    tunnel = None
    server = None
    logs = (scratch / "server.log").open("w")
    projects = []
    report = {"passed": False, "remote_run_dir": run_dir}
    begin = time.monotonic()
    try:
        report["image"] = json.loads(remote(shlex.join([python, run_dir + "/build_executor_image.py", "--package-dir", args.remote_package_dir, "--dockerfile", run_dir + "/Dockerfile"])))
        fixture = operator("start")
        api_port = free_port()
        tunnel = subprocess.Popen(ssh[:-1] + ["-N", "-o", "ExitOnForwardFailure=yes", "-L", f"127.0.0.1:{api_port}:127.0.0.1:{args.remote_port}", args.ssh], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        config = scratch / "opensandbox.json"
        private_json(config, {"base_url": f"http://127.0.0.1:{api_port}/", "api_key": fixture["api_key"], "image": report["image"]["image"], "executor_ssh_target": args.ssh, "allow_outer_sandbox_only": True})
        port = free_port()
        base = f"http://127.0.0.1:{port}"
        env = dict(os.environ, FEYNMAN_BIND=f"127.0.0.1:{port}", FEYNMAN_DATA_DIR=str(scratch / "data"), CODEX_HOME=str(home), FEYNMAN_CODEX_BIN=str(args.codex.resolve()), FEYNMAN_CODEX_MODEL="gpt-6-sol", FEYNMAN_OPENSANDBOX_CONFIG=str(config))
        for key in ["FEYNMAN_HOME", "FEYNMAN_DB", "FEYNMAN_WORKSPACE_ROOT", "FEYNMAN_OPENSANDBOX_SMOKE_CONFIG"]:
            env.pop(key, None)

        def start():
            process = subprocess.Popen([str(ROOT / "target/debug/feynman-server")], cwd=ROOT, env=env, stdout=logs, stderr=logs)
            wait_for(lambda: request(base, "/v1/health"))
            return process

        def cli(*args):
            result = subprocess.run([str(ROOT / "target/debug/feynman"), "--server", base, "--json", *args], cwd=ROOT, text=True, capture_output=True, timeout=180)
            if result.returncode:
                raise RuntimeError("CLI failed: " + result.stderr[-1500:] + result.stdout[-1500:])
            return json.loads(result.stdout)

        server = start()
        # Let the API tunnel/service settle without masking real HTTP failures.
        time.sleep(1)
        for name in ["Native Manager Smoke", "Isolated Peer Smoke"]:
            project = cli("projects", "create", "--name", name)["project"]["id"]
            projects.append(project)
            cli("workspace", "create", project, "--provider", "opensandbox")
            wait_for(lambda: cli("workspace", "refresh", project)["workspace"], lambda value: value["state"] == "running")
        project = projects[0]
        marker = secrets.token_hex(16)
        prompt = f"Authorized smoke. Use only native shell in the bound remote /workspace. Run python3 to write manager-probe.json containing {{'marker':'{marker}','hostname':socket.gethostname()}}. Read it back and report. Use no host tools or external-source tools."
        task = cli("tasks", "create", project, "--title", "Native remote write", "--prompt", prompt)["task"]
        task = wait_for(lambda: cli("tasks", "show", project, task["id"])["task"], lambda t: t["status"] in ["completed", "failed", "interrupted"], seconds=120)
        if task["status"] != "completed":
            raise RuntimeError("task failed: " + json.dumps(cli("runs", project, task["run_id"])))
        observed = resources("inspect", projects)["resources"]
        own = next(item for item in observed if item["project_id"] == project)
        peer = next(item for item in observed if item["project_id"] != project)
        assert own["probe"]["marker"] == marker and own["probe"]["hostname"] == own["hostname"]
        assert peer["probe"] is None and own["container_id"] != peer["container_id"]
        assert all(not item["model_auth_present"] for item in observed)
        before = cli("status", project)["main_agent"]["runtime_thread_id"]
        report.update({"projects": projects, "task_id": task["id"], "thread_id": before, "initial_container_id": own["container_id"], "isolation_verified": True})
        # Product pause/resume; pause retains memory/resources, not free scale-to-zero.
        cli("workspace", "stop", project)
        wait_for(lambda: cli("workspace", "refresh", project)["workspace"]["state"], lambda state: state == "paused")
        cli("workspace", "resume", project)
        wait_for(lambda: cli("workspace", "refresh", project)["workspace"]["state"], lambda state: state == "running")
        # Operator replacement simulates expiration/crash; product restore reuses the volume.
        resources("remove-container", [project])
        recovered = cli("send", project, "--message", "Use native shell to read /workspace/manager-probe.json without changing it. Report its marker.", "--wait", "--timeout-seconds", "120")
        assert recovered["run"]["status"] == "completed"
        restored = resources("inspect", [project])["resources"][0]
        assert restored["probe"] == own["probe"] and restored["container_id"] != own["container_id"]
        server.terminate()
        server.wait(timeout=15)
        server = start()
        follow = cli("send", project, "--message", "Use native shell to read /workspace/manager-probe.json and print it. Do not modify it. Report its marker.", "--wait", "--timeout-seconds", "120")
        assert follow["run"]["status"] == "completed"
        after = cli("status", project)
        assert before == after["main_agent"]["runtime_thread_id"]
        schedule = cli("schedules", "create", project, "--title", "One real scheduled observation", "--prompt", "Use native shell to read manager-probe.json in the bound Workspace and report its marker; do not modify anything.", "--at", "2026-01-01T00:00:00Z", "--enable")["schedule"]
        scheduled_tasks = wait_for(lambda: cli("tasks", "list", project)["tasks"], lambda tasks: any(t["schedule_id"] == schedule["id"] and t["status"] in ["completed", "failed", "interrupted"] for t in tasks), seconds=120)
        scheduled_task = next(t for t in scheduled_tasks if t["schedule_id"] == schedule["id"])
        assert scheduled_task["status"] == "completed"
        report.update({"replacement_container_id": restored["container_id"], "same_main_agent_thread_after_restart": True, "persistent_file_restored": True, "automatic_recovery_verified": True, "real_scheduled_task_id": scheduled_task["id"], "passed": True})
    except BaseException as error:
        report["error"] = str(error)
        raise
    finally:
        cleanup_errors = []
        if server and server.poll() is None:
            server.terminate()
            server.wait(timeout=15)
        try:
            if projects:
                report["cleanup"] = resources("cleanup", projects)
                assert report["cleanup"]["remaining_containers"] == 0
        except BaseException as error:
            cleanup_errors.append(str(error))
        try:
            if fixture:
                report["fixture_cleanup"] = operator("cleanup")
        except BaseException as error:
            cleanup_errors.append(str(error))
        if tunnel and tunnel.poll() is None:
            tunnel.terminate()
            tunnel.wait(timeout=15)
        (home / "auth.json").unlink(missing_ok=True)
        logs.close()
        if cleanup_errors:
            report["passed"] = False
            report["cleanup_errors"] = cleanup_errors
        report["elapsed_seconds"] = round(time.monotonic() - begin, 2)
        (scratch / "verification.json").write_text(json.dumps(report, indent=2))
        if cleanup_errors:
            raise RuntimeError("smoke cleanup failed; inspect evidence")
    print(json.dumps(report, indent=2), flush=True)


if __name__ == "__main__":
    main()
