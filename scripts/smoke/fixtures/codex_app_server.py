#!/usr/bin/env python3
"""Only the external Codex process is substituted in CLI/HTTP manager tests."""
import json
import os
from pathlib import Path
import sys
import time

if sys.argv[1:] == ['--version']:
    print('codex-cli 0.161.0')
    sys.exit(0)

log = Path(os.environ["FEYNMAN_TEST_CODEX_LOG"])
home = Path(os.environ["CODEX_HOME"])
sessions = home / "sessions"
sessions.mkdir(exist_ok=True)
with log.open("a") as stream:
    stream.write(json.dumps({"kind": "process_started", "codex_home": str(home), "sqlite_home": os.environ.get("CODEX_SQLITE_HOME"), "cwd": str(Path.cwd()), "inherited_model_credential_present":any(os.environ.get(k) for k in ["OPENAI_API_KEY","CODEX_API_KEY","CODEX_ACCESS_TOKEN"]), "managed_mcp_approval_mode":'mcp_servers.feynman_external_tools.default_tools_approval_mode="approve"' in sys.argv}) + "\n")
active = None


def emit(value):
    print(json.dumps(value), flush=True)


def complete():
    global active
    thread, turn = active
    emit({"method": "item/agentMessage/delta", "params": {"threadId": thread, "turnId": turn, "delta": "fixture reply"}})
    emit({"method": "turn/completed", "params": {"threadId": thread, "turn": {"id": turn, "status": "completed"}}})
    active = None


for line in sys.stdin:
    message = json.loads(line)
    with log.open("a") as stream:
        stream.write(json.dumps(message) + "\n")
    if message.get("id") == "fixture-approval" and "result" in message:
        complete()
        continue
    if "id" not in message or "method" not in message:
        continue
    method = message["method"]
    params = message.get("params", {})
    result = {}
    if method == "environment/info":
        result = {"cwd": "file:///workspace", "shell": {"name": "bash"}}
    elif method in ["thread/start", "thread/resume"]:
        thread = params.get("threadId", "fixture-" + Path.cwd().name)
        transcript = sessions / (thread + ".jsonl")
        if method == "thread/resume" and not transcript.is_file():
            emit({"id": message["id"], "error": {"code": -32000, "message": "stored thread missing from this CODEX_HOME"}})
            continue
        with transcript.open("a") as stream:
            stream.write(json.dumps({"fixture_thread": thread, "method": method}) + "\n")
        result = {"thread": {"id": thread}}
    elif method == "turn/start":
        if "FAIL_START" in params["input"][0]["text"]:
            emit({"id":message["id"],"error":{"code":-32000,"message":"fixture runtime startup failure"}})
            continue
        turn = "turn-" + str(message["id"])
        active = (params["threadId"], turn)
        result = {"turn": {"id": turn}}
    elif method == "turn/steer":
        if not active or params.get("expectedTurnId")!=active[1] or params.get("threadId")!=active[0]:
            emit({"id":message["id"],"error":{"code":-32000,"message":"no matching active turn"}})
            continue
        result={"turnId":active[1]}
    emit({"id": message["id"], "result": result})
    if method == "turn/start":
        if "STREAM_DELAY" in params["input"][0]["text"]:
            emit({"method":"item/agentMessage/delta","params":{"threadId":active[0],"turnId":active[1],"delta":"streaming... "}})
            time.sleep(1)
        if "DELAY" in params["input"][0]["text"]:
            time.sleep(2)
        if "BLOCK" in params["input"][0]["text"]:
            emit({"id": "fixture-approval", "method": "item/commandExecution/requestApproval", "params": {"threadId": active[0], "turnId": active[1], "command": "fixture inspection"}})
        elif "HOLD" not in params["input"][0]["text"]:
            complete()
    elif method=="turn/steer" and "FINISH" in params["input"][0]["text"]:
        complete()
