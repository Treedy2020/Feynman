"""Opt-in native Codex Home persistence check. No model turn or credential import.

Run after cargo build --workspace. Only temporary owned Home/Workspace is used.
"""
import json
import os
from pathlib import Path
import selectors
import shutil
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]


class NativeCodexHomeTest(unittest.TestCase):
    def test_native_thread_is_stored_and_resumed_from_owned_home(self):
        codex = os.environ.get("FEYNMAN_CODEX_BIN") or shutil.which("codex")
        self.assertIsNotNone(codex, "install native Codex before this opt-in smoke")
        with tempfile.TemporaryDirectory(prefix="feynman-native-home-") as directory:
            root = Path(directory)
            env = dict(os.environ, FEYNMAN_HOME=str(root / "feynman"))
            for key in ["FEYNMAN_DATA_DIR", "FEYNMAN_DB", "FEYNMAN_WORKSPACE_ROOT"]:
                env.pop(key, None)
            init = subprocess.run([str(ROOT / "target/debug/feynman"), "init"], cwd=root, env=env, text=True, capture_output=True, timeout=5)
            self.assertEqual(init.returncode, 0, init.stderr)
            home = Path(json.loads(init.stdout)["paths"]["codex_home"])
            env.update(CODEX_HOME=str(home), CODEX_SQLITE_HOME=str(home))
            work = root / "workspace"
            work.mkdir()

            def session(method, params):
                with (root / "native-stderr.log").open("a") as stderr:
                    process = subprocess.Popen([codex, "app-server"], cwd=work, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr, bufsize=0)
                    selector = selectors.DefaultSelector()
                    selector.register(process.stdout, selectors.EVENT_READ)
                    try:
                        def rpc(identifier, method, params):
                            process.stdin.write((json.dumps({"id": identifier, "method": method, "params": params}) + "\n").encode())
                            process.stdin.flush()
                            until = time.monotonic() + 20
                            while time.monotonic() < until:
                                if not selector.select(timeout=0.2):
                                    if process.poll() is not None:
                                        self.fail("native app-server exited before response")
                                    continue
                                line = process.stdout.readline()
                                self.assertTrue(line, "native app-server disconnected")
                                value = json.loads(line)
                                if value.get("id") == identifier:
                                    self.assertNotIn("error", value, "native RPC rejected " + method)
                                    return value["result"]
                            self.fail("native RPC timed out: " + method)

                        rpc(1, "initialize", {"clientInfo": {"name": "feynman_home_smoke", "version": "0.1.0"}, "capabilities": {"experimentalApi": True}})
                        process.stdin.write(b'{"method":"initialized"}\n')
                        process.stdin.flush()
                        result = rpc(2, method, params)
                        if method == "thread/start":
                            # Native Codex creates rollout storage lazily. Append
                            # a harmless raw history item without invoking a model.
                            rpc(3, "thread/inject_items", {"threadId": result["thread"]["id"], "items": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "OWNED_HOME_NATIVE_PROBE"}]}]})
                            until = time.monotonic() + 3
                            while not list((home / "sessions").rglob("*.jsonl")) and time.monotonic() < until:
                                time.sleep(0.05)
                        return result
                    finally:
                        selector.close()
                        process.terminate()
                        try:
                            process.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait(timeout=5)
                        process.stdin.close()
                        process.stdout.close()

            params = {"model": "gpt-6-sol", "cwd": str(work), "approvalPolicy": "on-request"}
            started = session("thread/start", params)
            thread = started["thread"]["id"]
            logs = list((home / "sessions").rglob("*.jsonl"))
            self.assertTrue(logs, "native thread must persist in the dedicated Home; path=" + str(started["thread"].get("path")) + "; ephemeral=" + str(started["thread"].get("ephemeral")))
            self.assertTrue(any(thread in path.name for path in logs))
            self.assertTrue(any("OWNED_HOME_NATIVE_PROBE" in path.read_text() for path in logs))
            resumed = session("thread/resume", dict(params, threadId=thread))
            self.assertEqual(resumed["thread"]["id"], thread)
            self.assertFalse((home / "auth.json").exists(), "check must not import personal credentials")


if __name__ == "__main__":
    unittest.main()
