#!/usr/bin/env python3
"""Offline native Git fixture: exercises the real fixed clone/askpass protocol."""
import json
import os
from pathlib import Path
import subprocess
import sys

TOKEN = 'FAKE_ACCESS_TOKEN_MUST_NOT_LEAK'
assert all(TOKEN not in arg for arg in sys.argv)
assert 'GITLAB_ACCESS_TOKEN' not in os.environ
assert 'DWS_CLIENT_SECRET' not in os.environ
assert os.environ['GIT_CONFIG_GLOBAL'] == '/dev/null'
assert os.environ['GIT_CONFIG_NOSYSTEM'] == '1'
assert os.environ['GIT_TERMINAL_PROMPT'] == '0'
args = sys.argv[1:]
for option in ('credential.helper=', 'http.followRedirects=false', 'core.hooksPath=/dev/null',
               'protocol.allow=never', 'protocol.https.allow=always'):
    assert option in args
if 'clone' in args:
    remote, destination = args[-2:]
    assert remote == 'https://gitlab.example/team/repo.git'
    assert '--no-checkout' in args and '--template=' in args
    helper = os.environ['GIT_ASKPASS']
    for kind, expected in [('Username', 'oauth2'), ('Password', TOKEN)]:
        result = subprocess.run([helper, f"{kind} for '{remote}': "], env=os.environ,
                                capture_output=True, text=True, check=True)
        assert result.stdout.strip() == expected
    for wrong in ('https://attacker.example/team/repo.git', 'https://gitlab.example/team/other.git',
                  'https://gitlab.example:8443/team/repo.git', remote+'?token=1'):
        result = subprocess.run([helper, f"Password for '{wrong}': "], env=os.environ,
                                capture_output=True, text=True)
        assert result.returncode and TOKEN not in result.stdout+result.stderr
    repo = Path(destination)
    (repo/'.git').mkdir()
    (repo/'.git/config').write_text(json.dumps({'url': remote}))
    (repo/'source.py').write_text('# fixture checkout\n')
elif 'checkout' in args:
    assert 'FEYNMAN_GIT_TOKEN' not in os.environ
    assert args[-1] == 'HEAD'
elif 'rev-parse' in args:
    assert 'FEYNMAN_GIT_TOKEN' not in os.environ
    print('a'*40)
else:
    raise ValueError('unexpected native Git command')
