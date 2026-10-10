"""Trusted workload launcher, delivered by the Server, never a model shell template.

Payload (including secrets) travels only on stdin. State/logs contain no payload.
Process groups are a cleanup mechanism for cooperative workloads, not containment.
"""
import json
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import sys
import time


def atomic(path, value):
    tmp = path.with_name(path.name + '.new')
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as f:
        json.dump(value, f)
    os.replace(tmp, path)


def directory(path):
    if path.is_symlink():
        raise ValueError('unsafe runtime directory')
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    if path.stat().st_uid != os.getuid():
        raise ValueError('runtime directory has another owner')
    path.chmod(0o700)
    return path


def read(path, fallback=None):
    if path.is_symlink():
        raise ValueError('unsafe runtime state')
    return json.loads(path.read_text()) if path.exists() else fallback


def supervise(payload, root):
    state = {'state': 'starting', 'instance': payload['instance'], 'cleanup': 'managed_process_group'}
    control = root / 'control.json'
    atomic(control, {'expires_at': payload['expires_at'], 'stop': False})
    expires = payload['expires_at']
    # Convert each bounded absolute expiry to a monotonic deadline; clock rollback cannot extend it.
    deadline = time.monotonic() + max(0, (expires - time.time() * 1000) / 1000)
    secret_values = [v for v in payload['secret_values'] if v]
    file_root = None
    child = None
    try:
        if time.monotonic() >= deadline:
            state['state'] = 'expired'
            atomic(root / 'status.json', state)
            return
        workspace = Path(payload['workspace']).resolve(strict=True)
        cwd = (workspace / payload.get('cwd', '')).resolve(strict=True)
        cwd.relative_to(workspace)
        env = {'PATH': '/opt/feynman/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin',
               'HOME': '/tmp', 'LANG': 'C.UTF-8'}
        env.update(payload['env'])
        if payload.get('files'):
            mounts = Path('/proc/mounts').read_text().splitlines()
            if not any(line.split()[1:3] == ['/dev/shm', 'tmpfs'] for line in mounts):
                raise ValueError('verified tmpfs required for secret files')
            file_root = directory(Path('/dev/shm') / ('feynman-' + payload['id']))
            for index, (name, value) in enumerate(payload['files'].items()):
                path = file_root / str(index)
                fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                with os.fdopen(fd, 'w') as f:
                    f.write(value)
                env[name] = str(path)
        child = subprocess.Popen(payload['argv'], cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                 start_new_session=True)
        state.update(state='running', pid=child.pid)
        atomic(root / 'status.json', state)
        selector = selectors.DefaultSelector()
        selector.register(child.stdout, selectors.EVENT_READ)
        buffer = b''
        patterns = sorted({v.encode() for v in secret_values}, key=len, reverse=True)
        written = 0

        def emit(data):
            nonlocal written
            for value in patterns:
                data = data.replace(value, b'[REDACTED]')
            if written < 262144:
                amount = data[:262144 - written]
                fd = os.open(root / 'output.log', os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
                with os.fdopen(fd, 'ab') as f:
                    f.write(amount)
                written += len(amount)

        terminal = 'completed'
        while True:
            cfg = read(control)
            if cfg['expires_at'] != expires:
                expires = cfg['expires_at']
                deadline = time.monotonic() + max(0, (expires - time.time() * 1000) / 1000)
            if cfg.get('stop') or time.monotonic() >= deadline:
                terminal = 'stopped' if cfg.get('stop') else 'expired'
                break
            for key, _ in selector.select(timeout=0.1):
                chunk = os.read(key.fileobj.fileno(), 8192)
                if chunk:
                    buffer += chunk
                    # Replace before splitting; retain a full longest-secret tail for split writes.
                    for value in patterns:
                        buffer = buffer.replace(value, b'[REDACTED]')
                    keep = 0
                    for value in patterns:
                        for length in range(1, min(len(buffer), len(value) - 1) + 1):
                            if buffer.endswith(value[:length]):
                                keep = max(keep, length)
                    cut = len(buffer) - keep
                    if cut:
                        emit(buffer[:cut]); buffer = buffer[cut:]
                else:
                    selector.unregister(key.fileobj)
            if child.poll() is not None:
                if not selector.get_map():
                    break
                # A descendant retaining stdout must not keep its completed parent alive forever.
                if not selector.select(timeout=0):
                    break
        # An unterminated secret prefix is withheld too; do not flush it on shutdown.
        emit(b'[REDACTED_PARTIAL]' if buffer and any(v.startswith(buffer) for v in patterns) else buffer)
        state.update(state=terminal, returncode=child.poll())
    except Exception:
        state['state'] = 'failed'  # No exception text: it may include a secret/path/argv.
    finally:
        if child:
            try:
                os.killpg(child.pid, signal.SIGTERM)
                until = time.monotonic() + 3
                while child.poll() is None and time.monotonic() < until:
                    time.sleep(0.05)
                # Dedicated owned process group only; no discovery or killing arbitrary host PIDs.
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=2)
            except Exception:
                state['cleanup'] = 'unknown'
            if child.stdout:
                child.stdout.close()
        if file_root:
            for path in file_root.iterdir():
                path.unlink()
            file_root.rmdir()
        atomic(root / 'status.json', state)


def main():
    payload = json.load(sys.stdin)
    if not re.fullmatch(r'work_[0-9a-f]{32}', payload['id']):
        raise ValueError('invalid workload id')
    root = directory(Path(payload['root'])) / payload['id']
    action = sys.argv[1]
    if action == 'supervise':
        supervise(payload, root)
        return
    if action == 'start':
        try:
            root.mkdir(mode=0o700)
        except FileExistsError:
            print(json.dumps(read(root / 'status.json', {'state': 'unknown'})))
            return
        child = subprocess.Popen([sys.executable, '-c', payload['_runner_code'], 'supervise'],
                                 stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL, start_new_session=True)
        child.stdin.write(json.dumps(payload).encode()); child.stdin.close()
        until = time.monotonic() + 3
        while not (root / 'status.json').exists() and time.monotonic() < until:
            time.sleep(0.02)
    if not root.exists():
        print(json.dumps({'state': 'missing'})); return
    directory(root)
    state = read(root / 'status.json', {'state': 'unknown'})
    if action in ('renew', 'stop') and state['state'] == 'running':
        cfg = read(root / 'control.json')
        if action == 'renew':
            if cfg.get('stop') or cfg['expires_at'] <= time.time() * 1000:
                raise ValueError('cannot revive stopped/expired workload')
            cfg['expires_at'] = payload['expires_at']
        else:
            cfg['stop'] = True
        atomic(root / 'control.json', cfg)
        if action == 'stop':
            until = time.monotonic() + 5
            while state['state'] == 'running' and time.monotonic() < until:
                time.sleep(0.05); state = read(root / 'status.json', {'state': 'unknown'})
    if action == 'logs':
        path = root / 'output.log'
        state['output'] = path.read_text(errors='replace')[-32768:] if path.exists() and not path.is_symlink() else ''
    print(json.dumps(state))


try:
    main()
except Exception:
    print(json.dumps({'state': 'failed', 'cleanup': 'unknown'}))
    sys.exit(1)
