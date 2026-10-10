"""Real CLI/HTTP/SQLite tests; only the upstream Codex process is substituted."""
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
import json
import base64
import hashlib
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import unittest
from test_dws_authorization import call, ROOT
from fixtures.opensandbox_lifecycle import upstream

FIXTURE = Path(__file__).parent / "fixtures/codex_app_server.py"


def wait_for(fn, predicate=lambda value: bool(value), seconds=10):
    until = time.monotonic() + seconds
    while True:
        value = fn()
        if predicate(value):
            return value
        if time.monotonic() > until:
            raise AssertionError("condition timed out: " + str(value))
        time.sleep(0.05)


@contextmanager
def platform(opensandbox=None, codex_override=None):
    with tempfile.TemporaryDirectory(prefix="feynman-manager-") as directory:
        root = Path(directory)
        fixture = root / "codex"
        shutil.copyfile(FIXTURE, fixture)
        fixture.chmod(0o700)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        base = f"http://127.0.0.1:{port}"
        env = dict(os.environ, FEYNMAN_BIND=f"127.0.0.1:{port}", FEYNMAN_DATA_DIR=str(root / "data"), FEYNMAN_CODEX_BIN=str(fixture), FEYNMAN_TEST_CODEX_LOG=str(root / "codex.jsonl"), OPENAI_API_KEY="PERSONAL_CREDENTIAL_CANARY", CODEX_API_KEY="PERSONAL_CREDENTIAL_CANARY", CODEX_ACCESS_TOKEN="PERSONAL_CREDENTIAL_CANARY")
        if codex_override is not None:
            env['FEYNMAN_CODEX_BIN'] = str(codex_override)
        for key in ["FEYNMAN_HOME", "FEYNMAN_DB", "FEYNMAN_WORKSPACE_ROOT", "FEYNMAN_OPENSANDBOX_CONFIG", "FEYNMAN_OPENSANDBOX_SMOKE_CONFIG"]:
            env.pop(key, None)
        if opensandbox:
            config = root / "opensandbox.json"
            config.write_text(json.dumps({"base_url": opensandbox, "api_key": "UPSTREAM_PRIVATE_KEY", "image": "feynman-codex-executor:0.157.1", "allow_outer_sandbox_only": True}))
            config.chmod(0o600)
            env["FEYNMAN_OPENSANDBOX_CONFIG"] = str(config)
        processes = []
        logs = (root / "server.log").open("w")

        def start():
            process = subprocess.Popen([str(ROOT / "target/debug/feynman-server")], cwd=ROOT, env=env, stdout=logs, stderr=logs)
            processes.append(process)
            wait_for(lambda: health(base))
            return process

        def stop():
            processes[-1].terminate()
            processes[-1].wait(timeout=10)

        start()
        try:
            yield base, root, start, stop
        finally:
            for process in processes:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)
            logs.close()


def health(base):
    try:
        return call(base, "GET", "/v1/health")[0] == 200
    except OSError:
        return False


def cli(base, *args, ok=True):
    result = subprocess.run([str(ROOT / "target/debug/feynman"), "--server", base, "--json", *args], cwd=ROOT, text=True, capture_output=True, timeout=15)
    if ok:
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)
    assert result.returncode != 0
    return result


def create(base, name):
    return cli(base, "projects", "create", "--name", name)["project"]


class ProjectManagerTests(unittest.TestCase):
    def test_wait_timeout_continues_the_same_run_without_resubmitting(self):
        with platform() as (base,root,start,stop):
            identifier=create(base,'Long model turn')['id']
            first=cli(base,'send',identifier,'--message','DELAY','--wait','--timeout-seconds','1')
            self.assertTrue(first['wait_timed_out'])
            self.assertEqual(first['run']['status'],'running')
            final=cli(base,'runs',identifier,first['run']['id'],'--wait','--timeout-seconds','5')
            self.assertEqual(final['run']['id'],first['run']['id'])
            self.assertEqual(final['run']['status'],'completed')
            self.assertEqual(len(cli(base,'conversation',identifier)['runs']),1)
            calls=[json.loads(line) for line in (root/'codex.jsonl').read_text().splitlines()]
            self.assertEqual(len([call for call in calls if call.get('method')=='turn/start']),1)

    def test_remote_seed_uses_the_execd_metadata_file_contract(self):
        with upstream() as (url,state), platform(url) as (base,root,start,stop):
            identifier = create(base,"Remote snapshot protocol")["id"]
            cli(base,"workspace","create",identifier,"--provider","opensandbox")
            snapshot = root/'snapshot.json'
            snapshot.write_text(json.dumps({'base_sha':'a'*40,'files':[{'path':'app.txt','executable':False,'content_base64':base64.b64encode(b'base').decode(),'sha256':hashlib.sha256(b'base').hexdigest()}],'facts':{'cutoff':'2026-09-01T00:00:00Z','messages':[]},'excluded_prefixes':[]}))
            result=cli(base,'workspace','seed',identifier,'--input',str(snapshot))
            self.assertEqual(result['seed']['file_count'],1)
            self.assertEqual(state.seed_upload['base_sha'],'a'*40)
            state.command_jsonl = False
            self.assertEqual(cli(base,'workspace','seed',identifier,'--input',str(snapshot))['seed']['file_count'],1)

    def test_runtime_uses_owned_auth_instead_of_inherited_harness_credentials(self):
        with platform() as (base, root, start, stop):
            project = create(base, "Owned company model identity")["id"]
            cli(base, "send", project, "--message", "Check owned identity", "--wait")
            started = [json.loads(line) for line in (root/"codex.jsonl").read_text().splitlines() if json.loads(line).get("kind")=="process_started"]
            self.assertEqual(len(started),1)
            self.assertFalse(started[0]["inherited_model_credential_present"])
            self.assertEqual(started[0]["codex_home"],str(root/"data/codex"))
            self.assertTrue(started[0]["managed_mcp_approval_mode"])

    def test_seed_initializes_only_a_new_idle_workspace_and_preserves_existing_files(self):
        with platform() as (base, root, start, stop):
            project = create(base, "Historical baseline")["id"]
            workspace = cli(base, "workspace", "create", project, "--provider", "local")["workspace"]
            bundle = root / "snapshot.json"
            bundle.write_text(json.dumps({"base_sha":"a"*40,"files":[{"path":"app.txt","executable":False,"content_base64":base64.b64encode(b"old baseline").decode(),"sha256":hashlib.sha256(b"old baseline").hexdigest()}],"facts":{"cutoff":"2026-09-01T00:00:00Z","messages":[]},"excluded_prefixes":[]}))
            original = json.loads(bundle.read_text())
            for unsafe in ('../outside.txt','.git/HEAD','nested/./app.txt','nested//app.txt'):
                invalid = json.loads(json.dumps(original))
                invalid['files'][0]['path']=unsafe
                bundle.write_text(json.dumps(invalid))
                cli(base,'workspace','seed',project,'--input',str(bundle),ok=False)
                self.assertFalse((Path(workspace['cwd'])/'seed-manifest.json').exists())
            invalid = json.loads(json.dumps(original));invalid['files'][0]['sha256']='0'*64
            bundle.write_text(json.dumps(invalid))
            cli(base,'workspace','seed',project,'--input',str(bundle),ok=False)
            self.assertFalse((Path(workspace['cwd'])/'repo').exists())
            bundle.write_text(json.dumps(original))
            result = cli(base, "workspace", "seed", project, "--input", str(bundle))
            self.assertEqual(result["seed"]["base_sha"], "a"*40)
            cwd = Path(workspace["cwd"])
            self.assertEqual((cwd / "repo/app.txt").read_text(), "old baseline")
            self.assertEqual(subprocess.check_output(["git", "-C", str(cwd / "repo"), "remote"], text=True), "")
            self.assertEqual(subprocess.check_output(["git", "-C", str(cwd / "repo"), "rev-list", "--count", "HEAD"], text=True).strip(), "1")
            cli(base, "workspace", "seed", project, "--input", str(bundle), ok=False)
            self.assertEqual((cwd / "repo/app.txt").read_text(), "old baseline")

    def test_active_remote_run_is_renewed_without_creating_another_turn(self):
        with upstream() as (url, state), platform(url) as (base, root, start, stop):
            project = create(base, "Active renewal")["id"]
            workspace = cli(base, "workspace", "create", project, "--provider", "opensandbox")["workspace"]
            blocked = cli(base, "send", project, "--message", "BLOCK", "--wait")
            expiry = (datetime.now(timezone.utc) + timedelta(seconds=400)).isoformat()
            state.records[workspace["sandbox_id"]]["expiresAt"] = expiry
            cli(base, "workspace", "refresh", project)
            wait_for(lambda: state.records[workspace["sandbox_id"]]["expiresAt"], lambda value: value != expiry, seconds=35)
            self.assertEqual(cli(base, "status", project)["main_agent"]["active_run"]["id"], blocked["run"]["id"])
            self.assertEqual(len(cli(base, "conversation", project)["runs"]), 1)
            self.assertNotIn("UPSTREAM_PRIVATE_KEY", (root / "codex.jsonl").read_text())

    def test_remote_reconciliation_resume_and_automatic_volume_recovery(self):
        with upstream() as (url, state), platform(url) as (base, root, start, stop):
            project = create(base, "Remote")["id"]
            state.ambiguous_once = True
            cli(base, "workspace", "create", project, "--provider", "opensandbox", ok=False)
            workspace = cli(base, "workspace", "create", project, "--provider", "opensandbox")["workspace"]
            self.assertEqual(len(state.creates), 1, "ambiguous POST must be reconciled, not repeated")
            self.assertNotIn("_", workspace["volume_name"])
            cli(base, "workspace", "stop", project)
            task = cli(base, "tasks", "create", project, "--title", "Queued", "--prompt", "Work")["task"]
            time.sleep(1.1)
            self.assertEqual(cli(base, "tasks", "show", project, task["id"])["task"]["status"], "queued")
            cli(base, "workspace", "resume", project)
            wait_for(lambda: cli(base, "tasks", "show", project, task["id"])["task"], lambda t: t["status"] == "completed")
            state.records.pop(workspace["sandbox_id"])
            stop()
            start()
            cli(base, "send", project, "--message", "Restore and continue", "--wait")
            restored = cli(base, "workspace", "show", project)["workspace"]
            self.assertNotEqual(workspace["sandbox_id"], restored["sandbox_id"])
            self.assertEqual(workspace["volume_name"], restored["volume_name"])
            self.assertEqual(state.creates[0]["volumes"], state.creates[1]["volumes"])
            messages = [json.loads(line) for line in (root / "codex.jsonl").read_text().splitlines()]
            for message in messages:
                if message.get("method") in ["thread/start", "thread/resume", "turn/start"]:
                    self.assertEqual(message["params"]["environments"], [{"environmentId": workspace["environment_id"], "cwd": "/workspace"}])
                    self.assertEqual(message["params"]["permissions"], ":danger-full-access")
            self.assertNotIn("UPSTREAM_PRIVATE_KEY", (root / "server.log").read_text())

    def test_remote_failures_never_start_local_codex_or_duplicate_workspaces(self):
        with upstream() as (url, state), platform(url) as (base, root, start, stop):
            project = create(base, "Fail closed")["id"]
            cli(base, "workspace", "create", project, "--provider", "opensandbox")
            for attribute in ["bad_endpoint", "bad_owner", "unavailable"]:
                setattr(state, attribute, True)
                cli(base, "send", project, "--message", "Do work", ok=False)
                status = cli(base, "status", project)
                self.assertEqual(status["main_agent"]["latest_run"]["status"], "failed")
                self.assertIsNone(status["main_agent"]["active_run"])
                self.assertFalse((root / "codex.jsonl").exists())
                setattr(state, attribute, False)
            self.assertEqual(len(state.creates), 1)
            public = cli(base, "workspace", "show", project)
            self.assertNotIn("UPSTREAM_PRIVATE_KEY", json.dumps(public))

    def test_second_server_cannot_interrupt_an_existing_database(self):
        with platform() as (base, root, start, stop):
            project = create(base, "Single scheduler")["id"]
            cli(base, "send", project, "--message", "BLOCK", "--wait")
            env = dict(os.environ, FEYNMAN_BIND="127.0.0.1:0", FEYNMAN_DB=str(root / "data/rust-platform.sqlite3"), FEYNMAN_DATA_DIR=str(root / "data"))
            result = subprocess.run([str(ROOT / "target/debug/feynman-server")], cwd=ROOT, env=env, text=True, capture_output=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("another Feynman server owns this database", result.stderr)
            self.assertEqual(cli(base, "status", project)["main_agent"]["active_run"]["status"], "waiting_approval")

    def test_skill_installation_is_idempotent_and_preserves_local_edits(self):
        with tempfile.TemporaryDirectory(prefix="feynman-skill-test-") as directory:
            value = cli("http://127.0.0.1:1", "skill", "install", "--skills-dir", directory)
            self.assertTrue(value["installed"])
            self.assertEqual(value, cli("http://127.0.0.1:1", "skill", "install", "--skills-dir", directory))
            skill = Path(directory) / "feynman/SKILL.md"
            skill.write_text("user modification")
            cli("http://127.0.0.1:1", "skill", "install", "--skills-dir", directory, ok=False)
            self.assertEqual(skill.read_text(), "user modification")

    def test_projects_tasks_and_restart_use_one_main_agent(self):
        with platform() as (base, root, start, stop):
            project = create(base, "One Main Agent")
            peer = create(base, "Isolated Peer")
            self.assertNotEqual(project["main_agent_id"], peer["main_agent_id"])
            workspace = cli(base, "workspace", "create", project["id"], "--provider", "local")["workspace"]
            other = cli(base, "workspace", "create", peer["id"], "--provider", "local")["workspace"]
            self.assertNotEqual(workspace["cwd"], other["cwd"])
            marker = Path(workspace["cwd"]) / "retained.txt"
            marker.write_text("persistent")
            task = cli(base, "tasks", "create", project["id"], "--title", "Test", "--prompt", "Do work")["task"]
            task = wait_for(lambda: cli(base, "tasks", "show", project["id"], task["id"])["task"], lambda t: t["status"] == "completed")
            self.assertIsNotNone(task["run_id"])
            before = cli(base, "status", project["id"])
            self.assertFalse(before["reviewer"]["required"])
            stop()
            start()
            response = cli(base, "send", project["id"], "--message", "Continue", "--wait")
            self.assertEqual(response["run"]["status"], "completed")
            after = cli(base, "status", project["id"])
            self.assertEqual(before["main_agent"]["runtime_thread_id"], after["main_agent"]["runtime_thread_id"])
            self.assertEqual(marker.read_text(), "persistent")
            self.assertEqual(len(cli(base, "status")["projects"]), 2)
            messages = [json.loads(line) for line in (root / "codex.jsonl").read_text().splitlines()]
            self.assertEqual(len([m for m in messages if m.get("method") == "thread/start"]), 1)
            self.assertEqual(len([m for m in messages if m.get("method") == "thread/resume"]), 1)
            starts = [m for m in messages if m.get("kind") == "process_started"]
            self.assertEqual(len(starts), 2)
            for process in starts:
                self.assertEqual(Path(process["codex_home"]), root / "data/codex")
                self.assertEqual(process["sqlite_home"], process["codex_home"])
            self.assertTrue(list((root / "data/codex/sessions").glob("*.jsonl")))
            self.assertEqual(call(base, "GET", f"/v1/projects/{peer['id']}/tasks/{task['id']}")[0], 404)

    def test_home_init_is_offline_and_auth_import_is_explicit(self):
        with tempfile.TemporaryDirectory(prefix="feynman-home-cli-") as directory:
            root = Path(directory)
            personal = root / "personal-codex"
            personal.mkdir()
            (personal / "auth.json").write_text('{"token":"DUMMY_PRIVATE_CACHE"}')
            (personal / "config.toml").write_text("# personal configuration")
            env = dict(os.environ, FEYNMAN_HOME=str(root / "feynman"), CODEX_HOME=str(personal), CODEX_SQLITE_HOME=str(personal))
            for key in ["FEYNMAN_DATA_DIR", "FEYNMAN_DB", "FEYNMAN_WORKSPACE_ROOT"]:
                env.pop(key, None)

            def offline(*args, ok=True):
                result = subprocess.run([str(ROOT / "target/debug/feynman"), "--server", "http://127.0.0.1:1", *args], cwd=root, env=env, text=True, capture_output=True, timeout=5)
                self.assertNotIn("DUMMY_PRIVATE_CACHE", result.stdout + result.stderr)
                self.assertEqual(result.returncode == 0, ok, result.stderr)
                return json.loads(result.stdout) if ok else result

            paths = offline("home")
            self.assertFalse(paths["initialized"])
            self.assertFalse((root / "feynman").exists())
            initial = offline("init")
            self.assertTrue(initial["initialized"])
            self.assertFalse(initial["auth_cache_present"])
            self.assertNotIn("personal configuration", (root / "feynman/codex/config.toml").read_text())
            offline("init", "--import-codex-auth", str(personal))
            copied = root / "feynman/codex/auth.json"
            self.assertFalse(copied.is_symlink())
            self.assertEqual(copied.stat().st_mode & 0o777, 0o600)
            self.assertEqual((root / "feynman").stat().st_mode & 0o777, 0o700)
            copied.write_text('{"token":"Feynman refresh only"}')
            offline("init", "--import-codex-auth", str(personal), ok=False)
            self.assertEqual((personal / "auth.json").read_text(), '{"token":"DUMMY_PRIVATE_CACHE"}')

    def test_missing_old_thread_fails_without_starting_a_new_conversation(self):
        with platform() as (base, root, start, stop):
            project = create(base, "Missing runtime history")["id"]
            cli(base, "send", project, "--message", "First", "--wait")
            before = cli(base, "status", project)["main_agent"]["runtime_thread_id"]
            stop()
            for transcript in (root / "data/codex/sessions").glob("*.jsonl"):
                transcript.unlink()
            start()
            cli(base, "send", project, "--message", "Resume", ok=False)
            self.assertEqual(cli(base, "status", project)["main_agent"]["runtime_thread_id"], before)
            messages = [json.loads(line) for line in (root / "codex.jsonl").read_text().splitlines()]
            self.assertEqual(len([m for m in messages if m.get("method") == "thread/start"]), 1)

    def test_busy_agent_queues_tasks_without_implicit_approval_and_recovers(self):
        with platform() as (base, root, start, stop):
            project = create(base, "Busy")["id"]
            blocked = cli(base, "send", project, "--message", "BLOCK", "--wait")
            self.assertEqual(blocked["run"]["status"], "waiting_approval")
            task = cli(base, "tasks", "create", project, "--title", "Next", "--prompt", "Follow up")["task"]
            time.sleep(1.2)
            self.assertEqual(cli(base, "tasks", "show", project, task["id"])["task"]["status"], "queued")
            cli(base, "send", project, "--message", "Conflict", ok=False)
            request = next(e["payload"]["requestId"] for e in blocked["events"] if e["kind"] == "approval_requested")
            cli(base, "approve", project, blocked["run"]["id"], request, "--decision", "accept")
            cli(base, "approve", project, blocked["run"]["id"], request, "--decision", "accept", ok=False)
            wait_for(lambda: cli(base, "tasks", "show", project, task["id"])["task"], lambda t: t["status"] == "completed")
            interrupted = cli(base, "tasks", "create", project, "--title", "Interrupt", "--prompt", "BLOCK")["task"]
            wait_for(lambda: cli(base, "status", project), lambda s: (s["main_agent"]["active_run"] or {}).get("status") == "waiting_approval")
            stop()
            start()
            self.assertEqual(cli(base, "tasks", "show", project, interrupted["id"])["task"]["status"], "interrupted")
            self.assertIsNone(cli(base, "status", project)["main_agent"]["active_run"])

    def test_schedule_paused_default_one_shot_and_resume(self):
        with platform() as (base, root, start, stop):
            project = create(base, "Schedules")["id"]
            plan = cli(base, "schedules", "create", project, "--title", "Observe", "--prompt", "Report", "--at", "2026-01-01T00:00:00+08:00")["schedule"]
            self.assertFalse(plan["enabled"])
            self.assertEqual(cli(base, "tasks", "list", project)["tasks"], [])
            cli(base, "schedules", "resume", project, plan["id"])
            messages = wait_for(lambda: cli(base, "inbox", "list", project)["inbox"], lambda t: len(t) == 1 and t[0]["state"] == "accepted")
            self.assertEqual(messages[0]["schedule_id"], plan["id"])
            self.assertEqual(cli(base,"runs",project,messages[0]["run_id"],"--wait")["run"]["status"],"completed")
            self.assertEqual(len(cli(base,"schedules","deliveries",project,plan["id"])["occurrences"]),1)
            stop()
            start()
            time.sleep(1.2)
            self.assertEqual(len(cli(base, "inbox", "list", project)["inbox"]), 1)
            cli(base, "schedules", "create", project, "--title", "Bad", "--prompt", "Report", "--every-seconds", "1", ok=False)
            cli(base, "schedules", "create", project, "--title", "Bad", "--prompt", "Report", "--at", "2026-01-01T00:00:00", ok=False)
            self.assertEqual(call(base, "POST", f"/v1/projects/{project}/workspace", {"provider": "opensandbox"})[0], 409)
            empty = create(base, "No remote configuration")["id"]
            self.assertEqual(call(base, "POST", f"/v1/projects/{empty}/workspace", {"provider": "opensandbox"})[0], 400)
            self.assertIsNone(cli(base, "workspace", "show", empty)["workspace"])


if __name__ == "__main__":
    unittest.main()
