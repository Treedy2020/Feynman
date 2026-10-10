"""Real Rust HTTP / CLI / MCP with offline credential and remote-CLI fixtures."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import tempfile
import time
import unittest
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler

ROOT = Path(__file__).resolve().parents[2]
TOKEN = "FAKE_ACCESS_TOKEN_MUST_NOT_LEAK"
OPENER = build_opener(ProxyHandler({}))

def call(base, method, path, body=None):
    request = Request(base + path, method=method, data=None if body is None else json.dumps(body).encode(), headers={} if body is None else {"Content-Type": "application/json"})
    try:
        response = OPENER.open(request, timeout=10)
    except HTTPError as error:
        response = error
    with response:
        payload = response.read()
        assert TOKEN.encode() not in payload
        return response.status, json.loads(payload) if payload else None

@contextmanager
def platform(include_process=False, native=False, opensandbox=None):
    with tempfile.TemporaryDirectory(prefix="feynman-access-test-") as directory:
        root = Path(directory)
        for name in ("glab", "git"):
            shutil.copyfile(Path(__file__).parent / "fixtures/access_cli.py", root / name)
            (root / name).chmod(0o700)
        shutil.copyfile(Path(__file__).parent / "fixtures/dws_device.py", root / "success")
        (root / "success").chmod(0o700)
        shutil.copyfile(Path(__file__).parent / "fixtures/codex_app_server.py", root / "codex")
        (root / "codex").chmod(0o700)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0)); port = sock.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        env = dict(os.environ, FEYNMAN_HOME=str(root / "data"), FEYNMAN_BIND=f"127.0.0.1:{port}", FEYNMAN_GLAB_BIN=str(root / "glab"), FEYNMAN_GIT_BIN=str(root / "git"), FEYNMAN_DWS_BIN=str(root / "success"), GITLAB_ACCESS_TOKEN="INHERITED_SECRET", DWS_CLIENT_SECRET="INHERITED_SECRET")
        env.update(FEYNMAN_CODEX_BIN=str(root/"codex"),FEYNMAN_TEST_CODEX_LOG=str(root/"codex.jsonl"))
        if native:
            fixtures = Path(__file__).parent/'fixtures'
            for name in ['native_python.py','native_drlee.py']:
                shutil.copyfile(fixtures/name,root/name);(root/name).chmod(0o700)
            shutil.copytree(fixtures/'native-package',root/'native-package')
            env.update(FEYNMAN_NATIVE_PYTHON=str(root/'native_python.py'),FEYNMAN_DRLEE_BIN=str(root/'native_drlee.py'),OPENAI_API_KEY='DO_NOT_INHERIT')
        for key in ("FEYNMAN_DATA_DIR", "FEYNMAN_DB", "FEYNMAN_WORKSPACE_ROOT", "FEYNMAN_GIT_ASKPASS", "FEYNMAN_OPENSANDBOX_CONFIG"):
            env.pop(key, None)
        if opensandbox:
            config = root/'opensandbox.json'
            config.write_text(json.dumps({'base_url':opensandbox,'api_key':'UPSTREAM_PRIVATE_KEY',
                'image':'feynman-codex-executor:0.161.0','allow_outer_sandbox_only':True}))
            config.chmod(0o600)
            env['FEYNMAN_OPENSANDBOX_CONFIG'] = str(config)
        log = root / "server.log"
        with log.open("w+") as output:
            process = subprocess.Popen([str(ROOT / "target/debug/feynman-server")], cwd=ROOT, env=env, stdout=output, stderr=output)
            try:
                deadline = time.monotonic() + 10
                while True:
                    try: call(base, "GET", "/v1/health"); break
                    except OSError:
                        if process.poll() is not None or time.monotonic() > deadline: raise AssertionError("server failed to start")
                        time.sleep(.02)
                a = call(base, "POST", "/v1/projects", {"name": "A"})[1]["project"]["id"]
                b = call(base, "POST", "/v1/projects", {"name": "B"})[1]["project"]["id"]
                values = (base, a, b, root, env)
                yield values + (process,) if include_process else values
            finally:
                process.terminate()
                try: process.wait(timeout=10)
                except subprocess.TimeoutExpired: process.kill(); process.wait(timeout=5)
        assert TOKEN not in log.read_text()

def register(base):
    status, body = call(base, "POST", "/v1/access/connections", {"provider": "gitlab", "hostname": "gitlab.example", "token": TOKEN})
    assert status == 200, body
    return body["connection"]["id"]

def bind(base, project, connection, provider="gitlab", repository="team/repo"):
    return call(base, "POST", f"/v1/projects/{project}/access/bindings", {"provider": provider, "connection_id": connection, "repository": repository})

def grant(base, project, tool):
    assert call(base, "PUT", f"/v1/projects/{project}/external-tools/{tool}")[0] == 201

def invoke(base, project, tool, arguments=None):
    return call(base, "POST", f"/v1/projects/{project}/access/invoke/{tool}", {"arguments": {} if arguments is None else arguments})

class ExternalAccessTest(unittest.TestCase):
    def test_dws_chat_requires_explicit_source_scope_and_respects_cutoff_through_mcp(self):
        with platform() as (base, a, b, root, env):
            grant(base, a, "dws.auth.device")
            auth = call(base, "POST", f"/v1/projects/{a}/dws-authorizations", {"reason": "Facts"})[1]["authorization"]
            deadline = time.monotonic() + 5
            while auth["status"] != "authorized":
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.03)
                auth = call(base, "GET", f"/v1/projects/{a}/dws-authorizations/{auth['id']}")[1]["authorization"]
            connection = call(base, "POST", "/v1/access/connections", {"provider": "dws", "project_id": a, "authorization_id": auth["id"]})[1]["connection"]["id"]
            self.assertEqual(call(base, "POST", f"/v1/projects/{a}/access/bindings", {"provider": "dws", "connection_id": connection})[0], 200)
            grant(base, a, "dws.chat.messages.read")
            arguments = {"target_kind": "group", "target_id": "cidFixture==", "start": "2026-09-01T00:00:00+08:00", "end": "2026-09-02T00:00:00+08:00"}
            self.assertEqual(invoke(base, a, "dws.chat.messages.read", arguments)[0], 409)
            scope = {"targets": [{"kind": "group", "id": "cidFixture=="}], "start": arguments["start"], "end": arguments["end"]}
            command = [str(ROOT / "target/debug/feynman"), "--server", base, "--json", "access", "dws-scope", a, "--scope-json", json.dumps(scope)]
            configured = subprocess.run(command, env=env, text=True, capture_output=True, timeout=10)
            self.assertEqual(configured.returncode, 0, configured.stderr)
            self.assertFalse(json.loads(configured.stdout)["business_grant_created"])
            result = invoke(base, a, "dws.chat.messages.read", arguments)
            self.assertEqual(result[0], 200, result)
            self.assertTrue(result[1]["data"]["complete"])
            self.assertEqual(result[1]["data"]["messages"][0]["messageId"], "fixture-message")
            self.assertNotIn("FIXTURE_SECRET_MUST_NOT_LEAK", json.dumps(result))
            self.assertEqual(invoke(base, a, "dws.chat.messages.read", dict(arguments, end="2026-09-03T00:00:00+08:00"))[0], 400)
            self.assertEqual(invoke(base, a, "dws.chat.messages.read", dict(arguments, target_id="cidOther=="))[0], 400)
            self.assertEqual(invoke(base, b, "dws.chat.messages.read", arguments)[0], 403)
            mcp_env = dict(env, FEYNMAN_DB=str(root / "data/rust-platform.sqlite3"), FEYNMAN_AGENT_PROJECT_ID=a, FEYNMAN_AGENT_USER_ID="local-user", FEYNMAN_AGENT_TENANT_ID="local", FEYNMAN_ACCESS_SERVER_URL=base)
            requests = [{"jsonrpc":"2.0", "id":1, "method":"initialize"}, {"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{"name":"dws_read_messages", "arguments":arguments}}]
            mcp = subprocess.run([str(ROOT / "target/debug/feynman-server"), "external-tools-mcp"], input="\n".join(json.dumps(r) for r in requests)+"\n", env=mcp_env, capture_output=True, text=True, timeout=10)
            response = json.loads(mcp.stdout.splitlines()[-1])["result"]
            self.assertFalse(response["isError"], response)
            call(base, "DELETE", f"/v1/projects/{a}/external-tools/dws.chat.messages.read")
            self.assertEqual(invoke(base, a, "dws.chat.messages.read", arguments)[0], 403)

    def test_dws_verified_connection_is_used_by_business_search(self):
        with platform() as (base, a, b, root, env):
            grant(base, a, "dws.auth.device")
            auth = call(base, "POST", f"/v1/projects/{a}/dws-authorizations", {"reason": "Facts"})[1]["authorization"]
            deadline = time.monotonic() + 5
            while auth["status"] != "authorized":
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.03)
                auth = call(base, "GET", f"/v1/projects/{a}/dws-authorizations/{auth['id']}")[1]["authorization"]
            response = call(base, "POST", "/v1/access/connections", {"provider": "dws", "project_id": a, "authorization_id": auth["id"]})
            self.assertEqual(response[0], 200, response)
            connection = response[1]["connection"]["id"]
            self.assertEqual(call(base, "POST", f"/v1/projects/{a}/access/bindings", {"provider": "dws", "connection_id": connection})[0], 200)
            grant(base, a, "dws.aisearch.enterprise")
            status, result = call(base, "POST", f"/v1/projects/{a}/external-tools/dws.aisearch.enterprise/invoke", {"arguments": {"queries": "Facts", "types": ["document"]}})
            self.assertEqual(status, 200, result)
            self.assertEqual(result["data"]["results"][0]["title"], "Bound fixture fact")
            self.assertNotIn("FIXTURE_SECRET_MUST_NOT_LEAK", json.dumps(result))
            # Explicitly share the account with B, then revoke A's login-start grant.
            # B's approved business access must not inherit or depend on A's grant.
            self.assertEqual(call(base, "POST", f"/v1/projects/{b}/access/bindings", {"provider": "dws", "connection_id": connection})[0], 200)
            grant(base, b, "dws.aisearch.enterprise")
            call(base, "DELETE", f"/v1/projects/{a}/external-tools/dws.auth.device")
            self.assertEqual(invoke(base, b, "dws.aisearch.enterprise", {"queries": "Facts"})[0], 200)

    def test_project_grants_binding_identity_and_redaction(self):
        with platform() as (base, a, b, root, env):
            self.assertEqual(len(call(base, "GET", "/v1/access/providers")[1]["providers"]), 5)
            connection = register(base)
            self.assertEqual(invoke(base, a, "gitlab.repo.read")[0], 403)
            grant(base, a, "gitlab.repo.read")
            self.assertEqual(invoke(base, a, "gitlab.repo.read")[0], 409)
            self.assertEqual(bind(base, a, connection)[0], 200)
            status, result = invoke(base, a, "gitlab.repo.read")
            self.assertEqual(status, 200)
            self.assertEqual(result["data"]["description"], "[redacted]")
            self.assertEqual(invoke(base, a, "gitlab.mr.read")[0], 403)
            grant(base, a, "gitlab.mr.read")
            self.assertEqual(invoke(base, a, "gitlab.mr.read")[1]["data"][0]["title"], "Fixture MR")
            self.assertEqual(invoke(base, b, "gitlab.repo.read")[0], 403)
            grant(base, b, "gitlab.repo.read")
            self.assertEqual(invoke(base, b, "gitlab.repo.read")[0], 409)
            bind(base, b, connection)
            call(base, "DELETE", f"/v1/projects/{a}/external-tools/gitlab.repo.read")
            self.assertEqual(invoke(base, b, "gitlab.repo.read")[0], 200)
            grant(base, a, "gitlab.repo.read")
            self.assertEqual(invoke(base, a, "gitlab.repo.read", {"argv": ["api", "user"]})[0], 400)
            self.assertEqual(bind(base, b, connection, repository="https://attacker.example/repo")[0], 400)
            (root / "data/private/external-access/glab/identity-changed").touch()
            self.assertEqual(invoke(base, a, "gitlab.repo.read")[0], 409)
            self.assertEqual(call(base, "POST", f"/v1/access/connections/{connection}/disable", {})[0], 200)
            self.assertEqual(invoke(base, a, "gitlab.repo.read")[0], 409)
            secret = root / f"data/private/external-access/{connection}.secret"
            self.assertEqual(secret.stat().st_mode & 0o777, 0o600)
            self.assertEqual(secret.read_text(), TOKEN)

    def test_git_native_cli_helper_and_unbind(self):
        with platform() as (base, a, b, root, env):
            connection = register(base)
            self.assertEqual(bind(base, a, connection, provider="git")[0], 200)
            grant(base, a, "git.remote.read")
            result = invoke(base, a, "git.remote.read")
            self.assertEqual(result[0], 200, result)
            self.assertEqual(result[1]["data"]["refs"][0]["ref"], "refs/heads/main")
            self.assertEqual(call(base, "DELETE", f"/v1/projects/{a}/access/bindings/git")[0], 204)
            self.assertEqual(invoke(base, a, "git.remote.read")[0], 409)
            self.assertEqual(call(base, "PUT", f"/v1/projects/{a}/external-tools/git.remote.push")[0], 400)

    def test_cli_requests_deduplicate_and_mcp_cannot_register_secret(self):
        with platform() as (base, a, b, root, env):
            cli = [str(ROOT / "target/debug/feynman"), "--server", base, "--json", "access"]
            result = subprocess.run(cli + ["connections", "register-gitlab", "--hostname", "gitlab.example", "--token-stdin"], input=TOKEN, capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertNotIn(TOKEN, result.stdout + result.stderr)
            connection = json.loads(result.stdout)["connection"]["id"]
            body = {"provider": "gitlab", "capability": "gitlab.repo.read", "reason": "Inspect repository"}
            first = call(base, "POST", f"/v1/projects/{a}/access/requests", body)[1]
            second = call(base, "POST", f"/v1/projects/{a}/access/requests", body)[1]
            self.assertEqual(first["request"]["id"], second["request"]["id"])
            self.assertEqual(call(base, "GET", f"/v1/projects/{b}/access")[1]["requests"], [])
            grant(base, a, "external.access")
            mcp_env = dict(env, FEYNMAN_DB=str(root / "data/rust-platform.sqlite3"), FEYNMAN_AGENT_PROJECT_ID=a, FEYNMAN_AGENT_USER_ID="local-user", FEYNMAN_AGENT_TENANT_ID="local", FEYNMAN_ACCESS_SERVER_URL=base)
            requests = [
                {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}},
                {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "feynman_access", "arguments": {"operation": "status"}}},
                {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "feynman_access", "arguments": {"operation": "register", "token": TOKEN}}},
            ]
            mcp = subprocess.run([str(ROOT / "target/debug/feynman-server"), "external-tools-mcp"], input="\n".join(json.dumps(item) for item in requests) + "\n", capture_output=True, text=True, env=mcp_env, timeout=10)
            self.assertEqual(mcp.returncode, 0, mcp.stderr)
            self.assertNotIn(TOKEN, mcp.stdout + mcp.stderr)
            responses = [json.loads(line) for line in mcp.stdout.splitlines()]
            self.assertTrue(responses[-1]["result"]["isError"])
            self.assertEqual(bind(base, a, connection)[0], 200)
            grant(base, a, "gitlab.repo.read")
            call(base, "DELETE", f"/v1/projects/{a}/external-tools/gitlab.repo.read")
            self.assertEqual(invoke(base, a, "gitlab.repo.read")[0], 403)

    def test_expiry_and_private_registry_reject_links(self):
        with platform() as (base, a, b, root, env):
            connection = register(base); bind(base, a, connection); grant(base, a, "gitlab.repo.read")
            database = root / "data/private/external-access/access.sqlite3"
            with sqlite3.connect(database) as db:
                row = db.execute("SELECT id,data FROM access_items WHERE kind='binding'").fetchone()
                binding = json.loads(row[1]); binding["expires_at"] = "2000-01-01T00:00:00Z"
                db.execute("UPDATE access_items SET data=? WHERE kind='binding' AND id=?", (json.dumps(binding), row[0]))
            self.assertEqual(invoke(base, a, "gitlab.repo.read")[0], 409)
            secret = root / f"data/private/external-access/{connection}.secret"
            secret.unlink(); secret.symlink_to(root / "unrelated")
            self.assertEqual(call(base, "GET", "/v1/access/connections")[0], 200)
            bind(base, a, connection)
            self.assertEqual(invoke(base, a, "gitlab.repo.read")[0], 500)

    def test_owner_http_rejects_browser_origin_and_unsafe_registration(self):
        with platform() as (base, a, b, root, env):
            request = Request(base + "/v1/access/providers", headers={"Origin": "https://attacker.example"})
            with self.assertRaises(HTTPError) as error:
                OPENER.open(request, timeout=10)
            self.assertEqual(error.exception.code, 401)
            error.exception.close()
            for host in ("https://gitlab.example", "gitlab.example:443", "user@gitlab.example", "gitlab.example/../secret", "-bad.example"):
                self.assertEqual(call(base, "POST", "/v1/access/connections", {"provider": "gitlab", "hostname": host, "token": TOKEN})[0], 400)
            connection = register(base)
            for repo in ("team/../repo", "team/-repo", "/team/repo", "team/repo;ls", "file:///tmp/repo", "team/repo?query"):
                self.assertEqual(bind(base, a, connection, repository=repo)[0], 400)
            self.assertEqual(call(base, "POST", f"/v1/projects/{a}/access/bindings", {"provider": "gitlab", "connection_id": connection, "repository": "team/repo", "ttl_seconds": 604801})[0], 400)
            self.assertEqual(call(base, "GET", "/v1/access/connections")[1]["connections"][0]["identity"], "42:fixture")

    def test_restart_keeps_connection_binding_request_and_cli_management(self):
        with platform(include_process=True) as (base, a, b, root, env, process):
            connection = register(base)
            command = [str(ROOT / "target/debug/feynman"), "--server", base, "--json", "access"]
            registered = subprocess.run(command + ["bind", a, "--provider", "gitlab", "--connection", connection, "--repository", "team/repo"], capture_output=True, text=True, timeout=10)
            self.assertEqual(registered.returncode, 0, registered.stderr)
            self.assertFalse(json.loads(registered.stdout)["business_grant_created"])
            grant(base, a, "gitlab.repo.read")
            request = call(base, "POST", f"/v1/projects/{a}/access/requests", {"provider": "gitlab", "capability": "gitlab.repo.read", "reason": "Persistent read"})[1]["request"]
            process.terminate(); process.wait(timeout=10)
            with (root / "restarted.log").open("w+") as log:
                restarted = subprocess.Popen([str(ROOT / "target/debug/feynman-server")], cwd=ROOT, env=env, stdout=log, stderr=log)
                try:
                    deadline = time.monotonic() + 5
                    while True:
                        try: call(base, "GET", "/v1/health"); break
                        except OSError:
                            self.assertLess(time.monotonic(), deadline)
                            time.sleep(.03)
                    status = subprocess.run(command + ["status", a], capture_output=True, text=True, timeout=10)
                    self.assertEqual(status.returncode, 0, status.stderr)
                    public = json.loads(status.stdout)
                    self.assertEqual(public["bindings"][0]["connection_id"], connection)
                    self.assertEqual(public["requests"][0]["request"]["id"], request["id"])
                    self.assertEqual(invoke(base, a, "gitlab.repo.read")[0], 200)
                    self.assertEqual(call(base, "GET", f"/v1/projects/{b}/access")[1]["bindings"], [])
                    unbound = subprocess.run(command + ["unbind", a, "--provider", "gitlab"], capture_output=True, text=True, timeout=10)
                    self.assertEqual(unbound.returncode, 0, unbound.stderr)
                    self.assertEqual(invoke(base, a, "gitlab.repo.read")[0], 409)
                finally:
                    restarted.terminate(); restarted.wait(timeout=10)
            self.assertNotIn(TOKEN, (root / "restarted.log").read_text())

if __name__ == "__main__":
    unittest.main()
