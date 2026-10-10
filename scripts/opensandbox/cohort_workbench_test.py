#!/usr/bin/env python3
"""Bounded operator checks for the explicit local C-01 historical project.

start authorizes one Schedule delivery; verify and capture are read-only in the
project. Evidence files are private and never overwritten. No prompt retries.
"""
import argparse
import base64
import json
from pathlib import Path
import sys
import time
import zlib

import local
from local_smoke import cli, owned_container
from edc_workbench import private_json

sys.path.insert(0, str(local.ROOT/'scripts/smoke'))
from project_manager_remote import VERIFY_CODE
from extract_final_proposal import save_final


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['verify', 'start', 'capture'])
    parser.add_argument('--home', type=Path, required=True)
    parser.add_argument('--evidence-dir', type=Path, required=True)
    parser.add_argument('--phase', choices=['before', 'after'], default='before')
    args = parser.parse_args()
    home = args.home.resolve(strict=True)
    out = args.evidence_dir.resolve(strict=True)
    assert not out.stat().st_mode & 0o077
    c = local.read_json(out/'configuration.json')
    project = c['project_id']
    assert cli(home, 'projects', 'show', project)['project']['name'] == 'Cohort · 历史迭代测试 C-01'
    workspace = cli(home, 'workspace', 'show', project)['workspace']
    container = owned_container(project, workspace)
    if args.action == 'verify':
        output = out/('integrity-'+args.phase+'.json')
        assert not output.exists()
        snapshot = local.read_json(out/'snapshot.json')
        expected = {'files': [{'path': f['path'], 'sha256': f['sha256']} for f in snapshot['files']],
                    'facts': snapshot['facts'], 'manifest': c['seed']}
        encoded = base64.b64encode(zlib.compress(json.dumps(expected).encode())).decode()
        result = local.run(['docker', 'exec', container['Id'], 'python3', '-c', VERIFY_CODE,
                            *[encoded[i:i+20000] for i in range(0, len(encoded), 20000)]],
                           capture_output=True, text=True)
        value = json.loads(result.stdout)
        private_json(output, value)
        print(json.dumps(value))
    elif args.action == 'start':
        assert (out/'integrity-before.json').exists() and not (out/'schedule-delivery.json').exists()
        assert not cli(home, 'conversation', project)['runs'], 'never duplicate a test turn'
        logs = cli(home, 'env', 'logs', project, c['bootstrap_lease']['id'])
        if logs.get('state') != 'completed' or logs.get('returncode') != 0:
            raise RuntimeError('toolchain bootstrap is not successfully completed')
        private_json(out/'bootstrap-logs.json', logs)
        for source in c['sources']:
            cli(home, 'sources', 'resume', project, source)
        cli(home, 'schedules', 'resume', project, c['schedule_id'])
        deadline = time.monotonic()+120
        try:
            while True:
                delivery = cli(home, 'schedules', 'deliveries', project, c['schedule_id'])
                if delivery['inbox'] and delivery['inbox'][0]['state'] == 'accepted':
                    private_json(out/'schedule-delivery.json', delivery)
                    conversation = cli(home, 'conversation', project)
                    private_json(out/'conversation-started.json', conversation)
                    assert len(conversation['runs']) == 1, 'unexpected duplicate delivery'
                    print(json.dumps({'run_id': conversation['runs'][0]['id'],
                        'inbox_state': 'accepted', 'occurrences': len(delivery['occurrences']),
                        'schedule_paused_after_acceptance': True}), flush=True)
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError('delivery timeout: inspect existing inbox; never resubmit')
                time.sleep(1)
        finally:
            cli(home, 'schedules', 'pause', project, c['schedule_id'])
    else:
        assert not (out/'run.json').exists()
        runs = cli(home, 'conversation', project)['runs']
        assert len(runs) == 1
        result = cli(home, 'runs', project, runs[0]['id'])
        assert result['run']['status'] == 'completed'
        private_json(out/'run.json', result)
        private_json(out/'final-provenance.json', save_final(result, home/'codex', out/'final-reply.txt'))
        for remote, filename in [('/workspace/output/cohort-iteration-proposal.md', 'cohort-iteration-proposal.md'),
                                 ('/workspace/scratch/offline-probe.py', 'offline-probe.py')]:
            content = local.run(['docker', 'exec', container['Id'], 'python3', '-c',
                                'from pathlib import Path;import sys;sys.stdout.write(Path(sys.argv[1]).read_text())',
                                remote], capture_output=True, text=True).stdout
            assert not (out/filename).exists()
            local.write_private(out/filename, content)
        private_json(out/'memory-after.json', cli(home, 'memory', 'search', project))
        private_json(out/'leases-after.json', cli(home, 'leases', 'list', project))
        private_json(out/'delivery-after.json', cli(home, 'schedules', 'deliveries', project, c['schedule_id']))
        print(json.dumps({'run': result['run']['id'], 'status': result['run']['status'],
                          'artifacts': str(out), 'original_stream_preserved': True}))


if __name__ == '__main__':
    main()
