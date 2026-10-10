"""Real Rust HTTP / MCP / CLI integration, with only the external DWS process substituted.

Run `cargo build --workspace` first, then:
python3 -m unittest discover -s scripts/smoke -p test_dws_authorization.py -v
No real login is started and no personal DWS/Codex profile is read.
"""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import unittest
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = Path(__file__).parent / "fixtures/dws_device.py"
OPENER = build_opener(ProxyHandler({}))


@contextmanager
def platform(mode):
    with tempfile.TemporaryDirectory(prefix="feynman-dws-auth-test-") as directory:
        root = Path(directory)
        binary = root / mode
        shutil.copyfile(FIXTURE, binary)
        binary.chmod(0o700)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        env = dict(os.environ, FEYNMAN_BIND=f"127.0.0.1:{port}",
                   FEYNMAN_DATA_DIR=str(root / "data"), FEYNMAN_DWS_BIN=str(binary),
                   DWS_CLIENT_SECRET="HOST_SECRET_MUST_NOT_BE_INHERITED")
        env.pop("FEYNMAN_DB", None)
        env.pop("FEYNMAN_HOME", None)
        env.pop("FEYNMAN_WORKSPACE_ROOT", None)
        log_path = root / "server.log"
        with log_path.open("w+") as logs:
            process = subprocess.Popen([str(ROOT / "target/debug/feynman-server")],
                                       cwd=ROOT, env=env, stdout=logs, stderr=logs)
            (root / "server.pid").write_text(str(process.pid))
            try:
                until = time.monotonic() + 10
                while True:
                    try:
                        call(base, "GET", "/v1/health")
                        break
                    except (OSError, HTTPError):
                        if process.poll() is not None or time.monotonic() > until:
                            raise AssertionError("server failed to start")
                        time.sleep(0.02)
                project = call(base, "POST", "/v1/projects", {"name": "Auth fixture"})[1]["project"]["id"]
                yield base, project, root, env
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            logs.flush()
            assert "FIXTURE_SECRET_MUST_NOT_LEAK" not in log_path.read_text()


def call(base, method, path, body=None):
    request = Request(base + path, method=method,
                      data=None if body is None else json.dumps(body).encode(),
                      headers={} if body is None else {"Content-Type": "application/json"})
    try:
        response = OPENER.open(request, timeout=5)
    except HTTPError as error:
        response = error
    with response:
        payload = response.read()
        assert b"FIXTURE_SECRET_MUST_NOT_LEAK" not in payload
        return response.status, json.loads(payload) if payload else None


def path(project, identifier=""):
    return f"/v1/projects/{project}/dws-authorizations" + ("/" + identifier if identifier else "")


def grant(base, project):
    assert call(base, "PUT", f"/v1/projects/{project}/external-tools/dws.auth.device")[0] == 201


def create(base, project):
    code, result = call(base, "POST", path(project), {"reason": "Read project facts"})
    assert code == 202, result
    return result["authorization"]


def wait_for(base, project, identifier, statuses):
    until = time.monotonic() + 8
    while time.monotonic() < until:
        code, result = call(base, "GET", path(project, identifier))
        assert code == 200, result
        record = result["authorization"]
        if record["status"] in statuses:
            return record
        time.sleep(0.02)
    raise AssertionError(f"authorization did not reach {statuses}: {record}")


class DwsAuthorizationIntegration(unittest.TestCase):
    def test_restart_preserves_request_and_interrupts_unfinished_flow(self):
        with platform("waiting") as (base, project, root, env):
            grant(base, project)
            record = create(base, project)
            wait_for(base, project, record["id"], {"waiting_for_user"})
            os.kill(int((root / "server.pid").read_text()), signal.SIGTERM)
            until = time.monotonic() + 5
            while time.monotonic() < until:
                try:
                    call(base, "GET", "/v1/health")
                except OSError:
                    break
                time.sleep(0.02)
            with (root / "restarted.log").open("w") as log:
                process = subprocess.Popen([str(ROOT / "target/debug/feynman-server")],
                                           env=env, cwd=ROOT, stdout=log, stderr=log)
                try:
                    until = time.monotonic() + 5
                    while time.monotonic() < until:
                        try:
                            result = wait_for(base, project, record["id"], {"interrupted"})
                            break
                        except OSError:
                            time.sleep(0.02)
                    else:
                        self.fail("restarted server did not recover the request")
                    self.assertIsNone(result["verification_uri"])
                    self.assertEqual(result["error_code"], "SERVER_RESTARTED")
                    self.assertEqual(len(call(base, "GET", path(project))[1]["authorizations"]), 1)
                finally:
                    process.terminate()
                    process.wait(timeout=5)

    def test_http_device_flow_links_deduplicates_and_verifies_identity_without_secrets(self):
        with platform("success") as (base, project, root, _):
            self.assertEqual(call(base, "POST", path(project), {"reason": "Facts"})[0], 403)
            grant(base, project)
            self.assertEqual(call(base, "POST", path(project), {"reason": "", "token": "spoof"})[0], 400)
            record = create(base, project)
            challenge = wait_for(base, project, record["id"], {"waiting_for_user"})
            self.assertEqual(challenge["user_code"], "ABCD-1234")
            self.assertEqual(challenge["verification_uri"], "https://login.dingtalk.com/device")
            self.assertEqual(challenge["verification_uri_complete"], "https://login.dingtalk.com/device?user_code=ABCD-1234")
            self.assertEqual(create(base, project)["id"], record["id"])
            result = wait_for(base, project, record["id"], {"authorized"})
            self.assertEqual(result["identity"]["corp_id"], "fixture-corp")
            self.assertIsNotNone(result["checked_at"])
            self.assertIsNone(result["verification_uri"])
            # A cross-origin simple/form POST must not trigger a token refresh.
            self.assertEqual(call(base, "POST", path(project, record["id"]) + "/check")[0], 400)
            self.assertEqual(call(base, "POST", path(project, record["id"]) + "/check", {})[1]["authorization"]["status"], "authorized")
            # OAuth success does not silently grant a business operation.
            tools = call(base, "GET", f"/v1/projects/{project}/external-tools")[1]["tools"]
            self.assertFalse(next(tool for tool in tools if tool["id"] == "dws.aisearch.enterprise")["granted"])
            other = call(base, "POST", "/v1/projects", {"name": "Other"})[1]["project"]["id"]
            self.assertEqual(call(base, "GET", path(other, record["id"]))[0], 404)
            database = (root / "data/rust-platform.sqlite3").read_bytes()
            self.assertNotIn(b"FIXTURE_SECRET_MUST_NOT_LEAK", database)

    def test_cancel_and_grant_revocation_stop_pending_authorization(self):
        with platform("waiting") as (base, project, root, _):
            grant(base, project)
            record = create(base, project)
            wait_for(base, project, record["id"], {"waiting_for_user"})
            self.assertEqual(call(base, "DELETE", path(project, record["id"]))[1]["authorization"]["status"], "cancelled")
            time.sleep(0.1)
            pid = int((root / "waiting.pid").read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
            record = create(base, project)
            wait_for(base, project, record["id"], {"waiting_for_user"})
            self.assertEqual(call(base, "DELETE", f"/v1/projects/{project}/external-tools/dws.auth.device")[0], 204)
            result = wait_for(base, project, record["id"], {"cancelled"})
            self.assertEqual(result["error_code"], "TOOL_GRANT_REVOKED")
            self.assertIsNone(result["user_code"])

    def test_expiry_denial_untrusted_links_and_bad_output_fail_closed(self):
        for mode, status, code in [("expired", "expired", "DEVICE_CODE_EXPIRED"),
                                   ("denied", "denied", "USER_DENIED"),
                                   ("untrusted", "failed", "UNTRUSTED_VERIFICATION_URI"),
                                   ("secret-uri", "failed", "UNTRUSTED_VERIFICATION_URI"),
                                   ("malformed", "failed", "DWS_INVALID_RESPONSE")]:
            with self.subTest(mode=mode), platform(mode) as (base, project, _, _):
                grant(base, project)
                record = create(base, project)
                result = wait_for(base, project, record["id"], {status})
                self.assertEqual(result["error_code"], code)
                self.assertIsNone(result["verification_uri"])

    def test_fresh_check_detects_invalidated_login_and_rejects_account_changes(self):
        for mode in ("invalidated", "changed"):
            with self.subTest(mode=mode), platform(mode) as (base, project, _, _):
                grant(base, project)
                record = create(base, project)
                wait_for(base, project, record["id"], {"authorized"})
                code, result = call(base, "POST", path(project, record["id"]) + "/check", {})
                if mode == "changed":
                    self.assertEqual(code, 500)
                else:
                    self.assertEqual(result["authorization"]["status"], "authentication_required")
                    self.assertEqual(result["authorization"]["identity"]["corp_id"], "fixture-corp")

    def test_real_mcp_bridge_uses_central_worker_and_cli_can_resume_status(self):
        with platform("waiting") as (base, project, root, env):
            grant(base, project)
            mcp_env = dict(env, FEYNMAN_DB=str(root / "data/rust-platform.sqlite3"),
                           FEYNMAN_ACCESS_SERVER_URL=base, FEYNMAN_AGENT_PROJECT_ID=project,
                           FEYNMAN_AGENT_TENANT_ID="local", FEYNMAN_AGENT_USER_ID="local-user")
            with subprocess.Popen([str(ROOT / "target/debug/feynman-server"), "external-tools-mcp"],
                                  env=mcp_env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True) as process:
                def rpc(message):
                    process.stdin.write(json.dumps(dict(jsonrpc="2.0", **message)) + "\n")
                    process.stdin.flush()
                    result = json.loads(process.stdout.readline())
                    self.assertNotIn("FIXTURE_SECRET_MUST_NOT_LEAK", json.dumps(result))
                    return result
                rpc({"id": 1, "method": "initialize"})
                tools = rpc({"id": 2, "method": "tools/list"})["result"]["tools"]
                self.assertFalse(next(tool for tool in tools if tool["name"] == "feynman_dws_authorization")["annotations"]["readOnlyHint"])
                result = rpc({"id": 3, "method": "tools/call", "params": {"name": "feynman_dws_authorization", "arguments": {"operation": "request", "reason": "Facts"}}})["result"]
                self.assertFalse(result["isError"], result)
                record = json.loads(result["content"][0]["text"])["data"]["authorization"]
                process.stdin.close()
                process.wait(timeout=5)
            challenge = wait_for(base, project, record["id"], {"waiting_for_user"})
            result = subprocess.run([str(ROOT / "target/debug/feynman"), "--server", base,
                                     "dws-auth", "status", project, record["id"]], capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn(challenge["verification_uri_complete"], result.stdout)
            self.assertIn(challenge["user_code"], result.stdout)
            call(base, "DELETE", path(project, record["id"]))


if __name__ == "__main__":
    unittest.main()
