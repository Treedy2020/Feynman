#!/usr/bin/env python3
"""Read-only continuation of an existing durable Run; never resubmits a prompt."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time
from run_project_manager_smoke import private_json

ROOT = Path(__file__).resolve().parents[2]

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--server',required=True)
    parser.add_argument('--project',required=True)
    parser.add_argument('--run',required=True)
    parser.add_argument('--evidence-dir',type=Path,required=True)
    parser.add_argument('--name',required=True)
    parser.add_argument('--max-seconds',type=int,default=1200)
    args=parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9-]+',args.name) or not 1<=args.max_seconds<=1800:
        parser.error('invalid name or time bound')
    if args.evidence_dir.is_symlink() or not args.evidence_dir.is_dir() or args.evidence_dir.stat().st_mode & 0o077:
        parser.error('evidence must be an existing private operator directory')
    output=args.evidence_dir/(args.name+'-continued-run.json')
    proposal=args.evidence_dir/(args.name+'-proposal.txt')
    if output.exists() or proposal.exists():
        parser.error('existing evidence is never replaced')
    deadline=time.monotonic()+args.max_seconds
    while True:
        value=subprocess.run([str(ROOT/'target/debug/feynman'),'--server',args.server,'--json','runs',args.project,args.run,'--wait','--timeout-seconds','30'],text=True,capture_output=True,timeout=45)
        if not value.stdout:
            raise RuntimeError('existing Run query failed; no prompt was resubmitted')
        result=json.loads(value.stdout)
        status=result['run']['status']
        if status not in ('created','queued','starting','running') or time.monotonic()>=deadline:
            private_json(output,result)
            text=''.join(e['payload']['text'] for e in result['events'] if e['kind']=='agent_text_delta')
            with open(proposal,'x',opener=lambda p,f:os.open(p,f,0o600)) as stream:
                stream.write(text)
            print(json.dumps({'status':status,'run':args.run,'evidence':str(output),'proposal':str(proposal),'prompt_resubmitted':False}),flush=True)
            if status!='completed':
                raise RuntimeError('Run is not completed; preserve owner-controlled runtime for inspection')
            return

if __name__=='__main__':
    main()
