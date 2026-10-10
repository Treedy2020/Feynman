"""Fixed provider bridge. Private stdout on owner import; public stdout on reads.

Only operator-selected executables/packages run here. No caller argv, HTTP path,
shell, writable business commands or user config fallback on Broker reads.
"""
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import webbrowser


def private_json(path):
    path = Path(path)
    stat = path.lstat()
    if path.is_symlink() or not path.is_file() or stat.st_mode & 0o077 or stat.st_size > 16384:
        raise ValueError('unsafe credential file')
    return json.loads(path.read_text())


def selected_account(bundle, host, handle=None):
    if bundle.get('version') != 1:
        raise ValueError('unsupported Drlee credential version')
    handle = handle or bundle.get('active_handle')
    accounts = [a for a in bundle['accounts'] if a['handle'] == handle]
    if len(accounts) != 1 or accounts[0]['endpoint']['root'].rstrip('/') != 'https://' + host:
        raise ValueError('account/endpoint mismatch')
    account = accounts[0]
    if not account.get('backend_user_id') or not account['credential'].get('access_token'):
        raise ValueError('missing Drlee identity/credential')
    return {'version': 1, 'active_handle': handle, 'accounts': [account]}


def scrub(value, secrets=()):
    secret_keys = {'token', 'accesstoken', 'refreshtoken', 'clientsecret',
                   'devicecode', 'authorizationcode', 'idtoken', 'password', 'credential'}
    if isinstance(value, dict):
        return {k: scrub(v, secrets) for k, v in value.items()
                if k.lower().replace('_', '').replace('-', '') not in secret_keys}
    if isinstance(value, list):
        return [scrub(v, secrets) for v in value]
    if isinstance(value, str):
        for secret in secrets:
            if secret:
                value = value.replace(secret, '[redacted]')
    return value


def slate_read(p):
    from slate_cli.cli import run
    from slate_cli.storage import InMemoryTokenStore
    commands = {'identity': ['me'], 'tasks': ['task', 'list', '--view', 'mine', '--limit', '20'],
                'projects': ['project', 'list']}
    out = io.StringIO()
    status = run(['--profile', 'prod', '--base-url', 'https://' + p['hostname'],
                  '--timeout', '10', '--retries', '0', '--json', *commands[p['operation']]],
                 token_store=InMemoryTokenStore({'prod': p['credential']}),
                 stdout=out, stderr=io.StringIO(), stdin=io.StringIO())
    if status:
        raise ValueError('Slate authentication/read failed')
    result = json.loads(out.getvalue())
    return scrub(result, [p['credential']])


def drlee_read(p):
    home = Path(p['home'])
    if home.is_symlink() or not home.is_dir() or home.stat().st_mode & 0o077:
        raise ValueError('unsafe Drlee home')
    path = home / 'accounts.json'
    if p['operation'] == 'register':
        bundle = selected_account(json.loads(p['credential']), p['hostname'])
        with open(path, 'x', opener=lambda name, flags: os.open(name, flags, 0o600)) as stream:
            json.dump(bundle, stream)
    before = selected_account(private_json(path), p['hostname'])
    account = before['accounts'][0]
    commands = {'register': ['auth'], 'identity': ['auth'],
                'channels': ['channel', 'list'],
                'messages': ['channel', 'messages', 'list', p.get('resource_id', ''), '--limit', '20'],
                'knowledge': ['knowledge', 'spaces', 'personal']}
    env = {k: os.environ[k] for k in ('PATH', 'HTTPS_PROXY', 'HTTP_PROXY', 'ALL_PROXY', 'NO_PROXY',
           'https_proxy', 'http_proxy', 'all_proxy', 'no_proxy') if k in os.environ}
    env.update(DRLEE_HOME=str(home), HOME=str(home), XDG_CONFIG_HOME=str(home / 'config'))
    def execute(args):
        result = subprocess.run([p['binary'], '--json', *args], env=env, cwd=home,
                                stdin=subprocess.DEVNULL, capture_output=True, timeout=25)
        if result.returncode or len(result.stdout) > 1048576:
            raise ValueError('Drlee operation failed')
        value = json.loads(result.stdout)
        if value.get('ok') is not True:
            raise ValueError('Drlee verification failed')
        return value
    # Online auth refreshes the privately copied token pair, never the user's cache.
    identity = execute(['auth'])
    if identity['data'].get('verified') is not True or identity['data'].get('email') != account['email'] or identity['data'].get('endpoint') != 'https://' + p['hostname']:
        raise ValueError('online Drlee identity/endpoint mismatch')
    current = selected_account(private_json(path), p['hostname'])
    if current['accounts'][0]['backend_user_id'] != account['backend_user_id']:
        raise ValueError('Drlee identity changed')
    value = identity if p['operation'] in ('register', 'identity') else execute(commands[p['operation']])
    after = selected_account(private_json(path), p['hostname'])
    if after['accounts'][0]['backend_user_id'] != account['backend_user_id']:
        raise ValueError('Drlee identity changed during read')
    secrets = [a['credential'].get(k) for a in (account, after['accounts'][0])
               for k in ('access_token', 'refresh_token')]
    return {'identity': str(account['backend_user_id']), 'data': scrub(value, secrets)}


def owner(p):
    mode = p['operation']
    if mode == 'import_gitlab':
        # Explicit owner import; stdout remains a private pipe to Feynman CLI.
        result = subprocess.run([p['binary'], 'config', 'get', 'token', '--host', p['hostname']],
                                capture_output=True, timeout=20, check=True)
        token = result.stdout.decode().strip()
        if not token or any(c.isspace() for c in token):
            raise ValueError('no native GitLab token')
        return {'credential': token}
    if mode in ('import_drlee', 'login_drlee'):
        if mode == 'import_drlee':
            path = Path(p['source_home']) / 'accounts.json'
            bundle = selected_account(private_json(path), p['hostname'], p.get('handle'))
        else:
            with tempfile.TemporaryDirectory(prefix='feynman-drlee-login-') as folder:
                env = dict(os.environ, DRLEE_HOME=folder)
                result = subprocess.run([p['binary'], '--json', 'auth', 'login', '--endpoint',
                                        'https://' + p['hostname']], env=env, stdout=subprocess.PIPE,
                                        timeout=300)
                if result.returncode:
                    raise ValueError('browser authorization incomplete')
                bundle = selected_account(private_json(Path(folder) / 'accounts.json'), p['hostname'])
        return {'credential': json.dumps(bundle)}
    if mode in ('login_slate', 'import_slate'):
        from slate_cli.cli import run
        from slate_cli.browser_auth import receive_cli_authorization
        from slate_cli.storage import InMemoryTokenStore, KeyringTokenStore
        if mode == 'import_slate':
            token = KeyringTokenStore().get(p['profile'])
            if not token:
                raise ValueError('no Slate Keychain session')
        else:
            store = InMemoryTokenStore()
            def opener(url):
                print('Feynman Slate authorization: ' + url, file=sys.stderr, flush=True)
                webbrowser.open(url)
                return True
            status = run(['--profile', 'prod', '--base-url', 'https://' + p['hostname'], '--json',
                          'auth', 'login', '--login-timeout', '300'], token_store=store,
                         stdout=io.StringIO(), stderr=io.StringIO(), stdin=io.StringIO(),
                         browser_login_reader=lambda url, timeout: receive_cli_authorization(url, timeout, opener=opener))
            if status:
                raise ValueError('Slate browser authorization incomplete')
            token = store.get('prod')
        return {'credential': token}
    raise ValueError('unsupported owner operation')


try:
    p = json.load(sys.stdin)
    if p['operation'].startswith(('import_', 'login_')):
        value = owner(p)
    elif p['provider'] == 'slate':
        value = slate_read(p)
    elif p['provider'] == 'drlee':
        value = drlee_read(p)
    else:
        raise ValueError('unsupported provider')
    print(json.dumps(value), flush=True)
except Exception:
    # Do not include upstream bodies, exception strings or credentials.
    print('Native CLI authentication/read unavailable; check package, login and endpoint.', file=sys.stderr)
    sys.exit(1)
