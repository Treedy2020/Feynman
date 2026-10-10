"""Public stdio-boundary regression checks for the diagnostic MCP bridge."""
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest


class SandboxReadBoundary(unittest.TestCase):
    def test_agent_cannot_read_outside_the_assigned_workspace(self):
        script = Path(__file__).with_name("gke_sandbox_mcp.py")
        requests = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                "name": "sandbox_read_file", "arguments": {"path": "../../etc/passwd"}}},
        ]
        env = dict(os.environ, FEYNMAN_SMOKE_POD="smoke", FEYNMAN_SMOKE_NAMESPACE="smoke",
                   FEYNMAN_SMOKE_KUBECONFIG="/nonexistent-smoke-config")
        run = subprocess.run([sys.executable, str(script)], env=env, input="".join(
            json.dumps(r) + "\n" for r in requests), text=True, capture_output=True, timeout=10)
        self.assertEqual(run.returncode, 0, run.stderr)
        replies = [json.loads(line) for line in run.stdout.splitlines()]
        self.assertTrue(replies[-1]["result"]["isError"])
        self.assertIn("outside", replies[-1]["result"]["content"][0]["text"])


if __name__ == "__main__":
    unittest.main()
