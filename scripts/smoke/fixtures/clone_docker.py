#!/usr/bin/env python3
"""Offline Docker transport. Only /workspace is mapped to the fixture directory.

Runs the actual fixed clone code; not a test of container isolation.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

root = Path(os.environ['FEYNMAN_TEST_CLONE_TRANSPORT'])
with (root/'docker-argv.jsonl').open('a') as stream:
    stream.write(json.dumps(sys.argv[1:])+'\n')
if sys.argv[1] == 'ps':
    print('fixture-container')
elif sys.argv[1] == 'inspect':
    owned = json.loads((root/'owned.json').read_text())
    print(json.dumps([{'Config':{'Labels':owned['labels']},'Mounts':[
        {'Type':'volume','Name':owned['volume'],'Destination':'/workspace'}]}]))
elif sys.argv[1] == 'exec':
    assert sys.argv[-1] == 'clone'
    code = sys.argv[-2]
    assert "Path('/workspace')" in code
    code = code.replace("Path('/workspace')", 'Path('+repr(str(root/'workspace'))+')')
    subprocess.run([sys.executable, '-c', code], input=sys.stdin.buffer.read(), check=True)
else:
    sys.exit(1)
