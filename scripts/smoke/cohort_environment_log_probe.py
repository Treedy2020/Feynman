#!/usr/bin/env python3
"""Read-only owner/Agent MCP comparison for one explicit local C-01 workload.

Returns red if a currently valid owned lease has owner-readable but Agent-denied
logs. Uses the same documented local stdio principal as the project runtime.
Never prints credentials or complete workload output and never launches a job.
"""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'opensandbox'))
import local
from local_smoke import cli


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--home', type=Path, required=True)
    p.add_argument('--project', required=True)
    p.add_argument('--workload', required=True)
    p.add_argument('--key', choices=['id', 'workload_id'], default='id')
    args = p.parse_args()
    home = args.home.resolve(strict=True)
    project = cli(home, 'projects', 'show', args.project)['project']
    assert project['name'] == 'Cohort · 历史迭代测试 C-01'
    leases = cli(home, 'leases', 'list', args.project)['items']
    matches = [x for x in leases if x['id'] == args.workload]
    assert len(matches) == 1
    lease = matches[0]['data']
    assert lease['agent_id'] == project['main_agent_id']
    valid = datetime.fromisoformat(lease['expires_at'].replace('Z', '+00:00')) > datetime.now(timezone.utc)
    assert valid, 'expiry would invalidate the owner/Agent comparison'
    owner = cli(home, 'env', 'logs', args.project, args.workload)
    assert owner['state'] == 'completed' and owner['returncode'] == 0
    environment = local.safe_env(home)
    assert (home/'rust-platform.sqlite3').is_file()
    environment.update({'FEYNMAN_AGENT_PROJECT_ID': args.project, 'FEYNMAN_AGENT_ID': project['main_agent_id'],
        'FEYNMAN_AGENT_TENANT_ID': 'local', 'FEYNMAN_AGENT_USER_ID': 'local-user',
        'FEYNMAN_DB': str(home/'rust-platform.sqlite3'),
        'FEYNMAN_ACCESS_SERVER_URL': 'http://127.0.0.1:4318'})
    calls = [
        {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
            'protocolVersion': '2024-11-05', 'capabilities': {},
            'clientInfo': {'name': 'cohort-readonly-log-probe', 'version': '1'}}},
        {'jsonrpc': '2.0', 'method': 'notifications/initialized', 'params': {}},
        {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/call', 'params': {
            'name': 'feynman_environment', 'arguments': {'operation': 'logs', 'arguments': {args.key: args.workload}}}}]
    result = subprocess.run([str(local.ROOT/'target/debug/feynman-server'), 'external-tools-mcp'],
        input=''.join(json.dumps(x)+'\n' for x in calls), env=environment,
        capture_output=True, text=True, timeout=15)
    assert result.returncode == 0, 'stdio probe failed; stderr intentionally not printed'
    responses = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    matched = [x for x in responses if x.get('id') == 2]
    assert len(matched) == 1, 'missing MCP reply'
    reply = matched[0]
    denied = bool(reply.get('error') or reply.get('result', {}).get('isError'))
    print(json.dumps({'lease_valid': valid, 'owner_logs_readable': True,
        'agent_logs_readable': not denied, 'parameter_key': args.key,
        'principal': 'same local project Agent as native runtime', 'workload_launched': False}))
    if denied:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
