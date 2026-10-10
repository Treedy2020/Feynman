"""CLI authentication uses only the Feynman-owned Codex home."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ModelAuthTest(unittest.TestCase):
    def test_login_status_and_logout_do_not_use_personal_home_or_expose_diagnostics(self):
        with tempfile.TemporaryDirectory(prefix="feynman-model-auth-") as directory:
            root = Path(directory)
            fake = root / "codex"
            fake.write_text("""#!/usr/bin/env python3
import os,sys
from pathlib import Path
p=Path(os.environ['CODEX_HOME'])
assert p == Path(os.environ['FEYNMAN_HOME'])/'codex'
assert os.environ['CODEX_SQLITE_HOME'] == str(p)
assert 'OPENAI_API_KEY' not in os.environ
if sys.argv[1:] == ['login','--device-auth']:
    (p/'auth.json').write_text('{}')
elif sys.argv[1:] == ['login','status']:
    print('secret=DO_NOT_EXPOSE',file=sys.stderr)
    sys.exit(0 if (p/'auth.json').exists() else 1)
elif sys.argv[1:] == ['logout']:
    (p/'auth.json').unlink(missing_ok=True)
else: sys.exit(2)
""")
            fake.chmod(0o700)
            env = dict(os.environ, FEYNMAN_HOME=str(root / "home"), FEYNMAN_CODEX_BIN=str(fake),
                       CODEX_HOME=str(root / "personal"), OPENAI_API_KEY="DO_NOT_INHERIT")
            env.pop("FEYNMAN_DATA_DIR", None)
            def cli(*args):
                result = subprocess.run([str(ROOT / "target/debug/feynman"), "--json", "auth", *args],
                                        env=env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn("DO_NOT_EXPOSE", result.stdout + result.stderr)
                return json.loads(result.stdout)
            self.assertFalse(cli("status")["authenticated"])
            self.assertTrue(cli("login", "--device")["authenticated"])
            self.assertTrue(cli("status")["authenticated"])
            self.assertFalse(cli("logout")["authenticated"])
            self.assertFalse((root / "personal").exists())


if __name__ == "__main__":
    unittest.main()
