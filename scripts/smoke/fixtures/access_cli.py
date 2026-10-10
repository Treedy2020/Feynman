#!/usr/bin/env python3
"""Offline glab/git processes. Exercise real CLI argv/env/askpass, no external accounts."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys

TOKEN = "FAKE_ACCESS_TOKEN_MUST_NOT_LEAK"
assert "DWS_CLIENT_SECRET" not in os.environ
assert "GITLAB_ACCESS_TOKEN" not in os.environ
if Path(sys.argv[0]).name == "glab":
    assert sys.argv[1:6] == ["api", "--hostname", "gitlab.example", "--method", "GET"]
    assert os.environ["GITLAB_TOKEN"] == TOKEN
    root = Path(os.environ["GLAB_CONFIG_DIR"])
    if (root / "identity-changed").exists():
        print(json.dumps({"id": 999, "username": "other"}))
    elif sys.argv[-1] == "user":
        print(json.dumps({"id": 42, "username": "fixture", "token": TOKEN}))
    elif "merge_requests" in sys.argv[-1]:
        print(json.dumps([{"iid": 1, "title": "Fixture MR", "state": "opened", "token": TOKEN}]))
    else:
        assert sys.argv[-1] == "projects/team%2Frepo"
        print(json.dumps({"id": 7, "path_with_namespace": "team/repo", "description": TOKEN, "token": TOKEN}))
else:
    assert "ls-remote" in sys.argv and "--heads" in sys.argv and "--tags" in sys.argv
    assert sys.argv[-1] == "https://gitlab.example/team/repo.git"
    assert os.environ["GIT_CONFIG_GLOBAL"] == "/dev/null"
    assert os.environ["GIT_TERMINAL_PROMPT"] == "0"
    helper = shlex.split(os.environ["GIT_ASKPASS"])
    for kind, expected in (("Username", "oauth2"), ("Password", TOKEN)):
        result = subprocess.run(helper + [f"{kind} for 'https://gitlab.example/team/repo.git': "], capture_output=True, text=True, env=os.environ, check=True)
        assert result.stdout.strip() == expected
    rejected = subprocess.run(helper + ["Password for 'https://attacker.example/team/repo.git': "], capture_output=True, text=True, env=os.environ)
    assert rejected.returncode != 0 and TOKEN not in rejected.stdout + rejected.stderr
    print("a" * 40 + "\trefs/heads/main")
