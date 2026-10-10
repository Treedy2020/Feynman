"""Fixed Git snapshot preparation through the public CLI."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]

class EvaluationSeedTest(unittest.TestCase):
    def test_preparation_excludes_future_refs_dirty_files_and_uses_a_fresh_repository(self):
        with tempfile.TemporaryDirectory(prefix="feynman-evaluation-test-") as directory:
            root = Path(directory)
            source = root / "source"
            source.mkdir()
            def git(*args):
                return subprocess.check_output(["git", "-C", str(source), *args], text=True).strip()
            git("init", "-q"); git("config", "user.name", "Fixture"); git("config", "user.email", "fixture@example.test")
            (source / "app.txt").write_text("old baseline")
            git("add", "."); git("commit", "-qm", "base")
            baseline = git("rev-parse", "HEAD")
            (source / "app.txt").write_text("future answer")
            git("add", "."); git("commit", "-qm", "future implementation")
            git("remote", "add", "origin", "https://example.test/private.git")
            (source / "dirty.txt").write_text("host private data")
            facts = root / "facts.json"
            facts.write_text('{"cutoff":"2026-09-01T00:00:00+08:00","messages":[]}')
            bundle = root / "input.json"
            command = [str(ROOT / "target/debug/feynman"), "evaluation", "prepare", "--repository", str(source), "--base-sha", baseline, "--facts", str(facts), "--output", str(bundle)]
            result = subprocess.run(command, text=True, capture_output=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            summary = json.loads(result.stdout)
            self.assertEqual(summary["base_sha"], baseline)
            self.assertTrue(bundle.exists())
            content = json.loads(bundle.read_text())
            self.assertEqual([f["path"] for f in content["files"]], ["app.txt"])
            self.assertNotIn("future answer", bundle.read_text())
            self.assertNotIn("dirty.txt", bundle.read_text())
            self.assertNotIn("private.git", bundle.read_text())
            self.assertEqual(bundle.stat().st_mode & 0o777, 0o600)
            again = subprocess.run(command, text=True, capture_output=True, timeout=15)
            self.assertNotEqual(again.returncode, 0, "must not overwrite a fixture")
            for index,invalid in enumerate([
                {"cutoff":"not-a-timestamp","messages":[]},
                {"cutoff":"2026-09-01T00:00:00Z","messages":[{"created_at":"2026-09-02T00:00:00Z","text":"future fact"}]},
            ]):
                facts.write_text(json.dumps(invalid))
                rejected = root / ("rejected-"+str(index)+".json")
                attempt = command[:-1]+[str(rejected)]
                failure = subprocess.run(attempt,text=True,capture_output=True,timeout=15)
                self.assertNotEqual(failure.returncode,0)
                self.assertFalse(rejected.exists())

if __name__ == "__main__":
    unittest.main()
