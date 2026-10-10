"""Native local Codex harness -> remote OpenSandbox Executor diagnostic."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import secrets
import shlex
import subprocess
import threading
import time

from run_opensandbox_smoke import free_port, request, wait_for

HERE = Path(__file__).resolve().parent
ENVIRONMENT = "feynman-smoke-native"


class Rpc:
    def __init__(self, scratch, binary):
        self.events = []
        self.inbox = queue.Queue()
        self.next_id = 0
        self.log = (scratch / "app-server.log").open("ab")
        self.event_log = (scratch / "native-events.jsonl").open("a")
        self.host_workspace = scratch / "host-workspace"
        self.host_workspace.mkdir(exist_ok=True)
        self.runtime_home = scratch / "codex-home"
        self.runtime_home.mkdir(mode=0o700, exist_ok=True)
        source_home = Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex-personal")))
        if not (source_home / "auth.json").is_file():
            raise RuntimeError("local Codex auth.json is required; no credentials are copied to the sandbox")
        (self.runtime_home / "auth.json").symlink_to(source_home / "auth.json")
        profile = ('permissions.native_smoke={extends=":workspace",filesystem={":root"="deny",'
                   '":minimal"="read",":workspace_roots"="write","/opt/codex"="read",'
                   + json.dumps(str(binary)) + '="read"},network={enabled=false}}')
        command = [str(binary), "app-server", "-c", 'model="gpt-6-sol"',
                   "-c", 'model_reasoning_effort="low"', "-c", 'default_permissions="native_smoke"', "-c", profile,
                   "-c", "mcp_servers={}", "-c", 'web_search="disabled"']
        for feature in ["plugins", "apps", "hooks", "browser_use", "computer_use", "image_generation", "skill_search"]:
            command += ["-c", "features." + feature + "=false"]
        command += ["-c", "features.skip_host_skill_discovery=true", "-c", "features.multi_agent=true"]
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
                                        text=True, bufsize=1, cwd=self.host_workspace,
                                        env={**os.environ, "CODEX_HOME": str(self.runtime_home)})
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        try:
            self.call("initialize", {"clientInfo": {"name": "feynman-native-smoke", "version": "1"}, "capabilities": {"experimentalApi": True}})
            self.send({"method": "initialized", "params": {}})
        except BaseException:
            self.close()
            raise

    def _read(self):
        for line in self.process.stdout:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            self.event_log.write(line)
            self.event_log.flush()
            self.inbox.put(event)
        self.inbox.put({"method": "fixture/exited"})

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def receive(self, timeout):
        event = self.inbox.get(timeout=timeout)
        if event.get("method") == "fixture/exited":
            raise RuntimeError("app-server exited; inspect app-server.log")
        self.events.append(event)
        if "id" in event and "method" in event:
            # Never silently approve an expanded permission request.
            self.send({"id": event["id"], "result": {"decision": "decline"}})
        return event

    def call(self, method, params, timeout=45):
        self.next_id += 1
        request_id = self.next_id
        self.send({"id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            event = self.receive(max(.1, deadline - time.monotonic()))
            if event.get("id") == request_id and "method" not in event:
                if "error" in event:
                    raise RuntimeError(method + ": " + json.dumps(event["error"]))
                return event["result"]
        raise RuntimeError(method + " timed out")

    def bind(self, port):
        self.call("environment/add", {"environmentId": ENVIRONMENT, "execServerUrl": f"ws://127.0.0.1:{port}", "connectTimeoutMs": 3000})
        return self.call("environment/info", {"environmentId": ENVIRONMENT})

    def turn(self, thread_id, prompt, timeout=180):
        start_index = len(self.events)
        result = self.call("turn/start", {"threadId": thread_id, "environments": [{"environmentId": ENVIRONMENT, "cwd": "/workspace"}],
                           "approvalPolicy": "never", "permissions": ":danger-full-access", "input": [{"type": "text", "text": prompt}]})
        turn_id = result["turn"]["id"]
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            event = self.receive(max(.1, deadline - time.monotonic()))
            if event.get("method") == "turn/completed" and event["params"]["threadId"] == thread_id and event["params"]["turn"]["id"] == turn_id:
                if event["params"]["turn"]["status"] != "completed":
                    raise RuntimeError("Agent turn failed: " + json.dumps(event["params"]["turn"]))
                events = self.events[start_index:]
                messages = [e["params"]["item"]["text"] for e in events if e.get("method") == "item/completed"
                            and e["params"].get("threadId") == thread_id and e["params"]["item"].get("type") == "agentMessage"
                            and e["params"]["item"].get("phase") == "final_answer"]
                if not messages:
                    messages = [i["text"] for i in event["params"]["turn"]["items"] if i.get("type") == "agentMessage" and i.get("phase") == "final_answer"]
                return "\n".join(messages), events
        raise RuntimeError("Agent turn timed out")

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.reader.join(timeout=5)
        self.event_log.close()
        self.log.close()
        (self.runtime_home / "auth.json").unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scratch", type=Path, required=True)
    parser.add_argument("--stage", choices=["main", "subagents", "isolation", "recovery"], default="main")
    parser.add_argument("--ssh", default="simonlong@spark-d304.tail242882.ts.net")
    parser.add_argument("--remote-runtime-dir", default="/tmp/feynman-opensandbox-smoke.tGx9qM")
    parser.add_argument("--remote-package-dir", default="/tmp/feynman-native-executor-LobzkDCF/package")
    parser.add_argument("--remote-port", type=int, default=18080)
    parser.add_argument("--codex", type=Path, default=Path("/opt/homebrew/Caskroom/codex/0.157.1/bin/codex"))
    args = parser.parse_args()
    scratch = args.scratch.resolve()
    scratch.mkdir(mode=0o700, parents=True, exist_ok=True)
    if list(scratch.iterdir()):
        raise RuntimeError("use an empty scratch directory")
    os.chmod(scratch, 0o700)
    ssh = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "StrictHostKeyChecking=yes", args.ssh]
    def remote(command, payload=None, timeout=90):
        result = subprocess.run(ssh + [command], input=None if payload is None else json.dumps(payload), capture_output=True, text=True, timeout=timeout)
        if result.returncode:
            raise RuntimeError("operator command failed: " + result.stderr[-2000:])
        return result.stdout.strip()
    run_dir = remote("mktemp -d /tmp/feynman-os-run-XXXXXXXX")
    for name in ["opensandbox_remote.py", "native_executor_remote.py"]:
        subprocess.run(["scp", "-q", "-o", "BatchMode=yes", str(HERE / name), args.ssh + ":" + run_dir + "/" + name], check=True)
    remote_python = args.remote_runtime_dir + "/venv/bin/python"
    def operator(action, payload=None):
        command = shlex.join([remote_python, run_dir + "/opensandbox_remote.py", action, "--run-dir", run_dir, "--port", str(args.remote_port)])
        return json.loads(remote(command, payload))
    def executor(action, sandbox_id, **payload):
        command = shlex.join([remote_python, run_dir + "/native_executor_remote.py", action, "--run-dir", run_dir, "--package-dir", args.remote_package_dir])
        return json.loads(remote(command, {"sandbox_id": sandbox_id, **payload}))
    tunnels = []
    fixture = None
    rpc = None
    report = {"passed": False, "stage": args.stage, "remote_run_dir": run_dir}
    begin = time.monotonic()
    def tunnel(port, address, remote_port):
        proc = subprocess.Popen(ssh[:-1] + ["-N", "-o", "ExitOnForwardFailure=yes", "-L", f"127.0.0.1:{port}:{address}:{remote_port}", args.ssh],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        tunnels.append(proc)
        return proc
    try:
        fixture = operator("start")
        api_port = free_port()
        tunnel(api_port, "127.0.0.1", args.remote_port)
        base = f"http://127.0.0.1:{api_port}"
        wait_for(lambda: request(base, "/v1/sandboxes", fixture["api_key"]), timeout=30)
        body = {"image": {"uri": "python:3.12-slim"}, "entrypoint": ["python3", "-c", "import time;time.sleep(1800)"],
                "resourceLimits": {"cpu": "1000m", "memory": "1Gi"}, "timeout": 1800, "metadata": {"feynman-smoke": fixture["run_id"]},
                "volumes": [{"name": "workspace", "pvc": {"claimName": fixture["volume"], "createIfNotExists": False, "deleteOnSandboxTermination": False}, "mountPath": "/workspace"}]}
        sandbox_id = request(base, "/v1/sandboxes", fixture["api_key"], "POST", body)["id"]
        wait_for(lambda: request(base, f"/v1/sandboxes/{sandbox_id}", fixture["api_key"])["status"]["state"] == "Running", timeout=90)
        marker = secrets.token_hex(24)
        canary = secrets.token_hex(24)
        independent = executor("prepare", sandbox_id, marker=marker, canary=canary)
        executor_port = free_port()
        executor_tunnel = tunnel(executor_port, independent["address"], 44773)
        rpc = Rpc(scratch, args.codex.resolve())
        info = wait_for(lambda: rpc.bind(executor_port), timeout=30)
        assert info["cwd"] == "file:///workspace", info
        thread = rpc.call("thread/start", {"cwd": str(rpc.host_workspace), "environments": [{"environmentId": ENVIRONMENT, "cwd": "/workspace"}],
                          "approvalPolicy": "never", "permissions": ":danger-full-access", "model": "gpt-6-sol",
                          "developerInstructions": "This is an authorized smoke fixture. Use native remote environment tools only. Never use MCP or host tools. Do not request elevated permissions."})
        thread_id = thread["thread"]["id"]
        report.update({"thread_id": thread_id, "environment_info": info, "sandbox_id": sandbox_id, "container_id": independent["container_id"],
                       "isolation": "unprivileged Docker/runc, no bind mounts; inner Codex profile :danger-full-access because bwrap namespaces are unavailable"})
        print("Native executor connected; main Agent smoke starting.", flush=True)
        session_nonce = secrets.token_hex(12)
        prompt = ("记住会话口令 " + session_nonce + "，不要把口令写入文件。本次无需返回口令。"
                  "在当前远端 /workspace 运行 pwd、uname -s、hostname，读取 probe.json。用原生 apply_patch 把 calculator.py 中加法实现修好，"
                  "然后运行 python3 -m unittest -v。不要调用 MCP 或派发子 Agent。最终只返回 JSON，字段为 cwd、os、hostname、marker、tests_passed。")
        answer, events = rpc.turn(thread_id, prompt)
        (scratch / "main-answer.txt").write_text(answer)
        observed = json.loads(answer.strip().removeprefix("```json").removesuffix("```").strip())
        assert observed["cwd"] == "/workspace" and observed["os"] == "Linux", observed
        assert observed["hostname"] == independent["hostname"] and observed["marker"] == marker, "not the seeded remote environment"
        assert observed["tests_passed"] is True
        check = executor("inspect", sandbox_id)
        assert "return a + b" in check["files"]["calculator.py"], check
        assert not list(rpc.host_workspace.iterdir()), "Agent wrote into local workspace"
        completed = [e["params"]["item"] for e in events if e.get("method") == "item/completed"]
        assert any(i["type"] == "commandExecution" and "unittest" in i.get("command", "") and i.get("exitCode") == 0 for i in completed), "missing successful native test execution"
        assert any(i["type"] == "fileChange" for i in completed), "missing native file change"
        report.update({"main_agent_remote_execution": True, "marker_sha256": hashlib.sha256(marker.encode()).hexdigest(), "main_native_item_types": sorted({i["type"] for i in completed}), "passed": True})
        print("Main Agent executed, patched and tested the remote workspace.", flush=True)
        if args.stage in ["subagents", "isolation", "recovery"]:
            report["passed"] = False
            prompt = ("请通过原生 spawn_agent 创建恰好两个子 Agent，先发出两个任务再等待它们完成。不要自己执行子任务。"
                      "子 Agent A：在继承的环境执行 pwd、uname -s、hostname，读取 /workspace/probe.json，把 JSON {cwd,os,hostname,marker,role:'A'} 写到 /workspace/subagent-a.json。"
                      "子 Agent B：独立执行相同观测，把 JSON {cwd,os,hostname,marker,role:'B'} 写到 /workspace/subagent-b.json，并运行 python3 -m unittest -v。"
                      "不要在任务里传递 marker 的值，让子 Agent 自己从文件读。等待两个任务全部完成，再仅返回 JSON {done:true}。不要使用 MCP。")
            answer, events = rpc.turn(thread_id, prompt)
            (scratch / "subagents-answer.txt").write_text(answer)
            calls = [e["params"]["item"] for e in events if e.get("method") == "item/completed" and e["params"]["item"]["type"] == "collabAgentToolCall"]
            activities = [e["params"]["item"] for e in events if e.get("method") == "item/completed" and e["params"]["item"]["type"] == "subAgentActivity"]
            child_ids = sorted({child for call in calls if call["tool"] == "spawnAgent" for child in call["receiverThreadIds"]}
                               | {i["agentThreadId"] for i in activities if i["kind"] == "started"})
            assert len(child_ids) == 2, "expected two native spawned subagents: " + json.dumps(calls)
            check = executor("inspect", sandbox_id)
            for role in ["a", "b"]:
                value = json.loads(check["files"]["subagent-" + role + ".json"])
                assert value["cwd"] == "/workspace" and value["os"] == "Linux"
                assert value["hostname"] == independent["hostname"] and value["marker"] == marker
                assert value["role"] == role.upper()
            child_evidence = {}
            for child_id in child_ids:
                child = rpc.call("thread/read", {"threadId": child_id, "includeTurns": True})
                child_evidence[child_id] = child
                child_items = [i for turn in child["thread"]["turns"] for i in turn["items"]]
                assert any(i["type"] == "commandExecution" and i["cwd"] == "/workspace" and i["exitCode"] == 0 for i in child_items), "missing remote child command"
                assert child["thread"]["environments"][0]["environmentId"] == ENVIRONMENT
                own_commands = [e["params"]["item"] for e in events if e.get("method") == "item/completed"
                                and e["params"].get("threadId") == child_id and e["params"]["item"]["type"] == "commandExecution"]
                assert any(i.get("exitCode") == 0 and independent["hostname"] in (i.get("aggregatedOutput") or "") for i in own_commands), "child must observe the remote host itself, not rely on inherited history"
            (scratch / "subagent-threads.json").write_text(json.dumps(child_evidence, indent=2))
            assert not list(rpc.host_workspace.iterdir()), "subagent wrote locally"
            report.update({"subagents_remote_execution": True, "subagent_thread_ids": child_ids, "passed": True})
            print("Two native subagents inherited the remote workspace and executed there.", flush=True)
        if args.stage in ["isolation", "recovery"]:
            report["passed"] = False
            host_canary = scratch / "host-canary.txt"
            host_canary.write_text(canary)
            host_canary.chmod(0o600)
            probe = ("import json\nr={}\n"
                     "for name,p in " + repr([("server", str(host_canary)), ("spark", run_dir + "/host-canary.txt")]) + ":\n"
                     " for mode in ['read','write']:\n"
                     "  try:\n"
                     "   with open(p,'r' if mode=='read' else 'r+') as f:\n"
                     "    f.read(1) if mode=='read' else f.write('MODIFIED')\n"
                     "   r[name+'_'+mode]=True\n"
                     "  except OSError:\n"
                     "   r[name+'_'+mode]=False\n"
                     "print(json.dumps(r))\n")
            prompt = ("只通过当前原生远端 Shell 执行以下 python3 脚本，这是两个已授权的 dummy canary 文件隔离测试；不要创建路径或改写脚本，"
                      "不要申请权限，不要派发子 Agent。最终仅返回脚本输出的 JSON：\n" + probe)
            answer, events = rpc.turn(thread_id, prompt)
            (scratch / "isolation-answer.txt").write_text(answer)
            expected_denials = {"server_read": False, "server_write": False, "spark_read": False, "spark_write": False}
            assert json.loads(answer) == expected_denials, "host canary accessible"
            commands = [e["params"]["item"] for e in events if e.get("method") == "item/completed" and e["params"]["item"]["type"] == "commandExecution"]
            assert any(i.get("exitCode") == 0 and json.loads(i.get("aggregatedOutput") or "{}") == expected_denials for i in commands), "missing native canary denial evidence"
            assert host_canary.read_text() == canary
            assert executor("inspect", sandbox_id)["host_canary_sha256"] == hashlib.sha256(canary.encode()).hexdigest()
            report.update({"host_file_isolation": True, "host_canary_reads_and_writes": expected_denials, "passed": True})
            print("Native remote commands cannot read or modify either host's dummy canary.", flush=True)
        if args.stage == "recovery":
            report["passed"] = False
            original_files = executor("inspect", sandbox_id)["files"]
            assert not any(session_nonce in value for value in original_files.values()), "session-only nonce leaked to workspace"
            executor_tunnel.terminate()
            executor_tunnel.wait(timeout=15)
            def disconnected():
                status = rpc.call("environment/status", {"environmentId": ENVIRONMENT})
                return status if status["status"] in ["disconnected", "pending"] else False
            report["offline_status"] = wait_for(disconnected, timeout=15)["status"]
            before = len(rpc.events)
            try:
                rpc.turn(thread_id, "尝试执行一次 uname -s；如果远端环境不可用就报告错误，不要重试，也不要切到其他环境。", timeout=45)
            except (RuntimeError, queue.Empty) as error:
                report["offline_error"] = str(error)
            offline_commands = [e["params"]["item"] for e in rpc.events[before:] if e.get("method") == "item/completed" and e["params"]["item"]["type"] == "commandExecution"]
            assert not any(i.get("exitCode") == 0 for i in offline_commands), "offline execution fell back to another environment"
            assert not list(rpc.host_workspace.iterdir())
            assert host_canary.read_text() == canary
            report["disconnect_fails_closed"] = True
            print("Disconnected executor fails closed; no successful host fallback.", flush=True)

            def verify_continuation(label, expected_hostname):
                prompt = ("继续当前会话。必须通过原生远端 Shell 新执行 pwd、uname -s、hostname、读取 /workspace/probe.json，"
                          "并运行 python3 -m unittest -v；不要派发子 Agent，不要使用缓存代替工具。"
                          "最终仅返回 JSON {cwd,os,hostname,marker,session_nonce,tests_passed}，session_nonce 是最初告诉你的会话口令，不能从文件获得。")
                answer, events = rpc.turn(thread_id, prompt)
                (scratch / (label + "-answer.txt")).write_text(answer)
                observed = json.loads(answer)
                assert observed["cwd"] == "/workspace" and observed["os"] == "Linux"
                assert observed["hostname"] == expected_hostname and observed["marker"] == marker
                assert observed["session_nonce"] == session_nonce and observed["tests_passed"] is True
                commands = [e["params"]["item"] for e in events if e.get("method") == "item/completed" and e["params"]["item"]["type"] == "commandExecution"]
                assert any(i.get("exitCode") == 0 and expected_hostname in (i.get("aggregatedOutput") or "") for i in commands)
                assert any(i.get("exitCode") == 0 and "unittest" in i.get("command", "") for i in commands)
                assert executor("inspect", sandbox_id)["files"] == original_files
                assert not list(rpc.host_workspace.iterdir())
                report[label] = True
                print(label + " passed with the same thread and remembered session nonce.", flush=True)

            executor_tunnel = tunnel(executor_port, independent["address"], 44773)
            rpc.bind(executor_port)
            verify_continuation("transport_recovery", independent["hostname"])
            executor("restart", sandbox_id)
            rpc.bind(executor_port)
            verify_continuation("executor_recovery", independent["hostname"])

            request(base, f"/v1/sandboxes/{sandbox_id}", fixture["api_key"], "DELETE")
            executor_tunnel.terminate()
            executor_tunnel.wait(timeout=15)
            recreate_begin = time.monotonic()
            sandbox_id = request(base, "/v1/sandboxes", fixture["api_key"], "POST", body)["id"]
            wait_for(lambda: request(base, f"/v1/sandboxes/{sandbox_id}", fixture["api_key"])["status"]["state"] == "Running", timeout=90)
            replacement = executor("prepare", sandbox_id)
            assert replacement["hostname"] != independent["hostname"]
            assert replacement["files"] == original_files, "persistent volume changed during replacement"
            executor_tunnel = tunnel(executor_port, replacement["address"], 44773)
            rpc.bind(executor_port)
            report["container_rebind_seconds"] = round(time.monotonic() - recreate_begin, 2)
            report["replacement_sandbox_id"] = sandbox_id
            report["replacement_container_id"] = replacement["container_id"]
            verify_continuation("container_recovery", replacement["hostname"])

            rpc.close()
            rpc = None
            rpc = Rpc(scratch, args.codex.resolve())
            rpc.bind(executor_port)
            resumed = rpc.call("thread/resume", {"threadId": thread_id, "approvalPolicy": "never", "permissions": ":danger-full-access", "excludeTurns": True})
            assert resumed["thread"]["id"] == thread_id
            verify_continuation("harness_session_recovery", replacement["hostname"])
            report.update({"session_nonce_sha256": hashlib.sha256(session_nonce.encode()).hexdigest(), "passed": True})
        native_items = [e["params"]["item"] for e in rpc.events if e.get("method") == "item/completed"]
        commands = [i for i in native_items if i["type"] == "commandExecution"]
        assert all(i["cwd"] == "/workspace" for i in commands), "native command escaped selected environment cwd"
        assert not any(i["type"] == "mcpToolCall" for i in native_items), "native smoke unexpectedly used MCP"
        report.update({"last_runtime_native_command_count": len(commands), "last_runtime_mcp_call_count": 0})
    except BaseException as error:
        report["passed"] = False
        report["error"] = str(error)
        raise
    finally:
        cleanup_errors = []
        try:
            if rpc:
                rpc.close()
        except BaseException as error:
            cleanup_errors.append("app-server: " + str(error))
        try:
            if fixture:
                report["cleanup"] = operator("cleanup")
                assert report["cleanup"]["remaining_containers"] == 0 and report["cleanup"]["remaining_volumes"] == 0
        except BaseException as error:
            cleanup_errors.append("remote fixture: " + str(error))
        finally:
            for proc in tunnels:
                try:
                    if proc.poll() is None:
                        proc.terminate()
                        proc.wait(timeout=15)
                except BaseException as error:
                    cleanup_errors.append("SSH tunnel: " + str(error))
            if cleanup_errors:
                report["passed"] = False
                report["cleanup_errors"] = cleanup_errors
            report["elapsed_seconds"] = round(time.monotonic() - begin, 2)
            (scratch / "verification.json").write_text(json.dumps(report, indent=2) + "\n")
        if cleanup_errors:
            raise RuntimeError("cleanup failed; inspect verification.json")
    print(json.dumps(report, indent=2), flush=True)


if __name__ == "__main__":
    main()
