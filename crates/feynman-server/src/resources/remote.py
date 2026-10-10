"""Operator-only Docker transport. No secret in Docker arguments or container metadata."""
import json
import subprocess
import sys
import uuid

try:
    p = json.load(sys.stdin)
    uuid.UUID(p['sandbox_id'])
    result = subprocess.run(['docker', 'ps', '--filter', 'label=opensandbox.io/id=' + p['sandbox_id'],
                             '--format', '{{.ID}}'], capture_output=True, timeout=8, check=True)
    ids = result.stdout.decode().splitlines()
    if len(ids) != 1:
        raise ValueError('sandbox unavailable/ambiguous')
    container = json.loads(subprocess.check_output(['docker', 'inspect', ids[0]], timeout=8))[0]
    if not any(m.get('Name') == p['volume'] and m['Type'] == 'volume' and m['Destination'] == '/workspace'
               for m in container['Mounts']):
        raise ValueError('workspace volume mismatch')
    labels = container['Config']['Labels']
    if any(labels.get(k) != p[k] for k in ['feynman-project', 'feynman-tenant', 'feynman-environment'] if k in p):
        raise ValueError('workspace ownership mismatch')
    timeout = p.get('control_timeout', 12)
    if type(timeout) is not int or not 1 <= timeout <= 120:
        raise ValueError('invalid fixed control timeout')
    result = subprocess.run(['docker', 'exec', '-i', ids[0], 'python3', '-c', p['code'], p['action']],
                            input=json.dumps(p['payload']).encode(), capture_output=True, timeout=timeout)
    if result.returncode:
        raise ValueError('runner failed')
    sys.stdout.buffer.write(result.stdout[:65536])
except Exception:
    print('{"state":"transport_failed"}')
    sys.exit(1)
