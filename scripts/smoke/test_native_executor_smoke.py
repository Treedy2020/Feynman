"""Opt-in real integration tests at the Codex app-server/environment seam."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


@unittest.skipUnless(os.environ.get("FEYNMAN_RUN_NATIVE_EXECUTOR_SMOKE") == "1", "requires Spark and local Codex authentication")
class NativeExecutorSmoke(unittest.TestCase):
    def test_main_agent_executes_edits_and_tests_in_remote_workspace(self):
        scratch = Path(tempfile.mkdtemp(prefix="feynman-native-smoke-"))
        print("\nEvidence: " + str(scratch), flush=True)
        command = ["python3", str(Path(__file__).with_name("run_native_executor_smoke.py")),
                   "--scratch", str(scratch), "--stage", "main"]
        run = subprocess.run(command, capture_output=True, text=True, timeout=300)
        self.assertEqual(run.returncode, 0, run.stderr[-2000:] + run.stdout[-2000:])
        report = json.loads((scratch / "verification.json").read_text())
        self.assertTrue(report["main_agent_remote_execution"])
        self.assertEqual(report["cleanup"]["remaining_containers"], 0)

    def test_subagents_inherit_the_same_remote_execution_environment(self):
        scratch = Path(tempfile.mkdtemp(prefix="feynman-native-smoke-"))
        print("\nEvidence: " + str(scratch), flush=True)
        command = ["python3", str(Path(__file__).with_name("run_native_executor_smoke.py")),
                   "--scratch", str(scratch), "--stage", "subagents"]
        run = subprocess.run(command, capture_output=True, text=True, timeout=300)
        self.assertEqual(run.returncode, 0, run.stderr[-2000:] + run.stdout[-2000:])
        report = json.loads((scratch / "verification.json").read_text())
        self.assertTrue(report["subagents_remote_execution"])
        self.assertEqual(len(report["subagent_thread_ids"]), 2)

    def test_remote_agent_cannot_access_host_canary_files(self):
        scratch = Path(tempfile.mkdtemp(prefix="feynman-native-smoke-"))
        print("\nEvidence: " + str(scratch), flush=True)
        command = ["python3", str(Path(__file__).with_name("run_native_executor_smoke.py")),
                   "--scratch", str(scratch), "--stage", "isolation"]
        run = subprocess.run(command, capture_output=True, text=True, timeout=300)
        self.assertEqual(run.returncode, 0, run.stderr[-2000:] + run.stdout[-2000:])
        report = json.loads((scratch / "verification.json").read_text())
        self.assertTrue(report["host_file_isolation"])

    def test_session_and_workspace_recover_without_local_fallback(self):
        scratch = Path(tempfile.mkdtemp(prefix="feynman-native-smoke-"))
        print("\nEvidence: " + str(scratch), flush=True)
        command = ["python3", str(Path(__file__).with_name("run_native_executor_smoke.py")),
                   "--scratch", str(scratch), "--stage", "recovery"]
        run = subprocess.run(command, capture_output=True, text=True, timeout=600)
        self.assertEqual(run.returncode, 0, run.stderr[-2500:] + run.stdout[-2500:])
        report = json.loads((scratch / "verification.json").read_text())
        self.assertTrue(report["disconnect_fails_closed"])
        self.assertTrue(report["transport_recovery"])
        self.assertTrue(report["executor_recovery"])
        self.assertTrue(report["container_recovery"])
        self.assertTrue(report["harness_session_recovery"])


if __name__ == "__main__":
    unittest.main()
