#!/usr/bin/env python3
"""Offline Docker CLI fixture: protocol test, NOT a container isolation test."""
import json
import os
from pathlib import Path
import subprocess
import sys

root=Path(os.environ['FEYNMAN_TEST_WORKLOAD_TRANSPORT'])
with (root/'docker-argv.jsonl').open('a') as log:
    log.write(json.dumps(sys.argv[1:])+'\n')
if sys.argv[1]=='ps':
    print('fixture-container')
elif sys.argv[1]=='inspect':
    owned=json.loads((root/'owned.json').read_text()) if (root/'owned.json').exists() else {
        'labels':{'feynman-project':'fixture-project','feynman-tenant':'local','feynman-environment':'fixture-environment'},
        'volume':'fixture-volume'}
    print(json.dumps([{'Config':{'Labels':owned['labels']},'Mounts':[{'Type':'volume','Name':owned['volume'],'Destination':'/workspace'}]}]))
elif sys.argv[1]=='exec':
    payload=json.load(sys.stdin)
    payload['root']=str(root/'runtime')
    payload['workspace']=str(root/'workspace')
    subprocess.run([sys.executable,'-c',sys.argv[-2],sys.argv[-1]],input=json.dumps(payload).encode(),check=True)
else:
    sys.exit(1)
