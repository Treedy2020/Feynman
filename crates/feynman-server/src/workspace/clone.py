"""Credential-bearing fixed Git process inside the owned Sandbox, never host checkout."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import uuid


def clone(p):
    host, repo, token = p['hostname'], p['repository'], p['token']
    key = str(uuid.UUID(p['idempotency_key']))
    if not re.fullmatch(r'[a-zA-Z0-9.-]+', host) or not all(
            re.fullmatch(r'[a-zA-Z0-9_.-]+', v) and v not in ('.', '..') and not v.startswith('-')
            for v in repo.split('/')):
        raise ValueError('invalid clone destination')
    root = Path('/workspace')
    destination = root / 'repo'
    receipt = root / '.feynman-clone.json'
    remote = f'https://{host}/{repo}.git'
    if destination.exists() or destination.is_symlink():
        if receipt.is_file() and not receipt.is_symlink():
            old = json.loads(receipt.read_text())
            if old['idempotency_key'] == key and old['remote'] == remote:
                return dict(old, state='already_completed')
        raise ValueError('repository already exists; never overwrite workspace data')
    if receipt.exists() or receipt.is_symlink():
        raise ValueError('existing receipt')
    # Temporary helper contains code only. Credential travels in the Git process
    # environment, not URL/argv/repo config/Docker metadata. Trusted root Agent can
    # inspect process environment: this is NOT a hostile-agent secrecy boundary.
    with tempfile.TemporaryDirectory(prefix='feynman-git-') as folder:
        helper = Path(folder) / 'askpass'
        helper.write_text('''#!/usr/bin/env python3
import os, sys
from urllib.parse import urlsplit
prompt = sys.argv[1]
parts = prompt.split("'")
if len(parts) < 3: sys.exit(1)
actual, expected = urlsplit(parts[1]), urlsplit(os.environ['FEYNMAN_GIT_REMOTE'])
if (actual.scheme, actual.hostname, actual.port, actual.path) != (expected.scheme, expected.hostname, expected.port, expected.path) or actual.query or actual.fragment or actual.password: sys.exit(1)
if prompt.startswith('Username for '): print('oauth2')
elif prompt.startswith('Password for '): print(os.environ['FEYNMAN_GIT_TOKEN'])
else: sys.exit(1)
''')
        helper.chmod(0o700)
        env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'HOME': folder,
               'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
               'GIT_TERMINAL_PROMPT': '0', 'GIT_ASKPASS': str(helper),
               'FEYNMAN_GIT_TOKEN': token, 'FEYNMAN_GIT_REMOTE': remote}
        fixed = ['git', '-c', 'credential.helper=', '-c', 'credential.useHttpPath=true',
                 '-c', 'http.followRedirects=false', '-c', 'core.hooksPath=/dev/null',
                 '-c', 'protocol.allow=never', '-c', 'protocol.https.allow=always']
        destination.mkdir(mode=0o755)  # Exclusive reservation; failed partial clone retained.
        subprocess.run([*fixed, 'clone', '--quiet', '--no-checkout', '--template=', '--',
                        remote, str(destination)], env=env, stdin=subprocess.DEVNULL,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=55, check=True)
        # No submodules, LFS smudge/config, project hooks, install scripts or app startup.
        env.pop('FEYNMAN_GIT_TOKEN')
        subprocess.run([*fixed, '-C', str(destination), 'checkout', '--quiet', 'HEAD'], env=env,
                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=8, check=True)
        head = subprocess.check_output([*fixed, '-C', str(destination), 'rev-parse', 'HEAD'],
                                       env=env, stderr=subprocess.DEVNULL, timeout=3).decode().strip()
        if not re.fullmatch(r'[a-f0-9]{40,64}', head):
            raise ValueError('invalid head')
        result = {'idempotency_key': key, 'remote': remote, 'path': '/workspace/repo',
                  'head': head, 'state': 'cloned', 'push_authorized': False,
                  'credential_persisted': False}
        with receipt.open('x') as stream:
            json.dump(result, stream)
        return result


try:
    print(json.dumps(clone(json.load(sys.stdin))), flush=True)
except Exception:
    # No raw Git stderr, credentials or exception string. Failed clone is not retried
    # with destructive cleanup; inspect retained workspace through normal file tools.
    print('{"state":"clone_failed_or_existing","overwritten":false}', flush=True)
    sys.exit(1)
