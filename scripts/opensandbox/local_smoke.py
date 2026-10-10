#!/usr/bin/env python3
"""Real local Docker smoke. --agent additionally uses the owned company model identity."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
import secrets
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid

import local


def cli(home, *arguments, timeout=90):
    profile=local.settings(home)
    result=local.run([local.ROOT/'target/debug/feynman','--server',
                     f"http://127.0.0.1:{profile['server_port']}",'--json',*arguments],
                    env=local.safe_env(home),capture_output=True,text=True,timeout=timeout)
    return json.loads(result.stdout)


def owned_container(project, workspace):
    ids=local.run(['docker','ps','--filter','label=opensandbox.io/id='+workspace['sandbox_id'],
                   '--format','{{.ID}}'],capture_output=True,text=True).stdout.splitlines()
    assert len(ids)==1,'owned smoke container unavailable'
    container=json.loads(local.run(['docker','inspect',ids[0]],capture_output=True,text=True).stdout)[0]
    labels=container['Config']['Labels']
    assert labels['feynman-project']==project and labels['feynman-tenant']=='local'
    assert labels['feynman-environment']==workspace['environment_id']
    assert any(m.get('Name')==workspace['volume_name'] and m['Destination']=='/workspace' for m in container['Mounts'])
    assert not any(m['Type']=='bind' for m in container['Mounts'])
    host=container['HostConfig']
    assert not host['Privileged'] and 'ALL' in host['CapDrop']
    assert 'no-new-privileges:true' in host['SecurityOpt']
    assert all(b['HostIp']=='127.0.0.1' for values in host['PortBindings'].values() for b in values)
    return container


def inspect(project, workspace):
    container=owned_container(project,workspace)
    code="""import json,os,socket
from pathlib import Path
root=Path('/workspace')
print(json.dumps({'hostname':socket.gethostname(),'marker':(root/'repo/marker.txt').read_text(),
 'env_probe':json.loads((root/'scratch/env-probe.json').read_text()) if (root/'scratch/env-probe.json').exists() else None,
 'agent_probe':json.loads((root/'output/agent-probe.json').read_text()) if (root/'output/agent-probe.json').exists() else None,
 'credentials_absent':not any(p.exists() for p in [Path('/root/.codex/auth.json'),Path('/root/.dws')]),
 'model_keys_absent':not any(os.environ.get(k) for k in ['OPENAI_API_KEY','CODEX_API_KEY','CODEX_ACCESS_TOKEN'])}))
"""
    return json.loads(local.run(['docker','exec',container['Id'],'python3','-c',code],capture_output=True,text=True).stdout)


def verify_proxy(project, workspace, config):
    container=owned_container(project,workspace)
    port=container['HostConfig']['PortBindings']['44772/tcp'][0]['HostPort']
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    statuses=[]
    for url,headers in [(f'http://127.0.0.1:{port}/health',{}),
                        (config['base_url']+'v1/sandboxes/'+workspace['sandbox_id']+'/proxy/44772/health',
                         {'OPEN-SANDBOX-API-KEY':config['api_key']})]:
        try:
            with opener.open(urllib.request.Request(url,headers=headers),timeout=8) as response:
                statuses.append(response.status)
        except urllib.error.HTTPError as error:
            statuses.append(error.code)
    assert statuses[0]==statuses[1] and statuses[0]<500,'local proxy must preserve direct execd HTTP status'


def wait(fn, predicate, seconds=60):
    until=time.monotonic()+seconds
    while True:
        value=fn()
        if predicate(value):
            return value
        if time.monotonic()>until:
            raise RuntimeError('smoke condition timed out; preserve the project for inspection')
        time.sleep(.25)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--home',type=Path,default=Path.home()/'.feynman-local-sandbox')
    parser.add_argument('--agent',action='store_true',help='Explicitly authorize a bounded real model turn')
    parser.add_argument('--project',help='Retry only an unused Local Sandbox Smoke project; never reuse a business project')
    parser.add_argument('--restore',action='store_true',help='Replace ONLY the newly created smoke container, preserving its volume')
    args=parser.parse_args();home=args.home.resolve(strict=True)
    config=local.read_json(home/'runtime/opensandbox.json')
    assert config['executor_ssh_target'] is None,'this smoke is local-only'
    if args.project:
        project=args.project
        assert cli(home,'projects','show',project)['project']['name']=='Local Sandbox Smoke'
        assert not cli(home,'conversation',project)['runs'],'retry requires no previous Agent Runs'
    else:
        project=cli(home,'projects','create','--name','Local Sandbox Smoke')['project']['id']
    print('Local smoke project: '+project,file=sys.stderr,flush=True)
    workspace=cli(home,'workspace','create',project,'--provider','opensandbox')['workspace']
    workspace=wait(lambda:cli(home,'workspace','refresh',project)['workspace'],lambda w:w['state']=='running')
    verify_proxy(project,workspace,config)
    marker=secrets.token_hex(12)
    files={'marker.txt':marker,'calculator.py':'def add(a, b):\n    return a + b\n',
           'test_calculator.py':'import unittest\nfrom calculator import add\nclass Addition(unittest.TestCase):\n    def test_add(self):\n        self.assertEqual(add(2, 3), 5)\n'}
    seed={'base_sha':'a'*40,'files':[{'path':name,'executable':False,
          'content_base64':base64.b64encode(content.encode()).decode(),
          'sha256':hashlib.sha256(content.encode()).hexdigest()} for name,content in files.items()],
          'facts':{'cutoff':'2026-10-09T00:00:00Z','messages':[]},'excluded_prefixes':[]}
    seed_path=home/'runtime'/('smoke-seed-'+secrets.token_hex(6)+'.json')
    local.write_private(seed_path,json.dumps(seed))
    try:
        container=owned_container(project,workspace)
        exists=local.run(['docker','exec',container['Id'],'python3','-c',
                         "from pathlib import Path;print(int(Path('/workspace/seed-manifest.json').exists()))"],
                        capture_output=True,text=True).stdout.strip()=='1'
        if not exists:
            cli(home,'workspace','seed',project,'--input',str(seed_path))
        else:
            assert args.project,'new smoke workspace must be empty'
            code="import json;from pathlib import Path;p=Path('/workspace/repo');print(json.dumps({n:(p/n).read_text() for n in ['marker.txt','calculator.py','test_calculator.py']}))"
            retained=json.loads(local.run(['docker','exec',container['Id'],'python3','-c',code],capture_output=True,text=True).stdout)
            assert retained['calculator.py']==files['calculator.py'] and retained['test_calculator.py']==files['test_calculator.py']
            assert re.fullmatch(r'[a-f0-9]{24}',retained['marker.txt']),'not the synthetic smoke baseline'
            marker=retained['marker.txt']
    finally:
        seed_path.unlink(missing_ok=True)
    observed=inspect(project,workspace)
    assert observed['marker']==marker and observed['credentials_absent'] and observed['model_keys_absent']
    cli(home,'workspace','stop',project)
    assert cli(home,'workspace','refresh',project)['workspace']['state']=='paused'
    cli(home,'workspace','resume',project)
    workspace=wait(lambda:cli(home,'workspace','refresh',project)['workspace'],lambda w:w['state']=='running')
    assert inspect(project,workspace)['marker']==marker
    profile={'name':'local-smoke','argv':['python3','-c',
         "import json,os,socket;from pathlib import Path;Path('/workspace/scratch/env-probe.json').write_text(json.dumps({'mode':os.environ['APP_MODE'],'hostname':socket.gethostname()}));print('local env delivered')"],
         'variables':{'APP_MODE':'local-smoke'},'secrets':{},'secret_files':{},'max_seconds':30}
    profile_path=home/'runtime'/('smoke-profile-'+secrets.token_hex(6)+'.json')
    local.write_private(profile_path,json.dumps({'profile':profile}))
    try:
        cli(home,'env','put',project,'--input',str(profile_path))
    finally:
        profile_path.unlink(missing_ok=True)
    request=cli(home,'env','request',project,'--profile','local-smoke','--reason','Synthetic local Docker smoke','--seconds','30')['request']
    cli(home,'env','approve',project,request['id'],'--revision',str(request['revision']),'--accept-secret-exposure','--seconds','120')
    lease=cli(home,'env','run',project,request['id'],'--idempotency-key',str(uuid.uuid4()))['lease']
    def current_lease():
        return next(item for item in cli(home,'leases','list',project)['items'] if item['id']==lease['id'])
    wait(current_lease,lambda l:l['data']['state']=='completed')
    observed=inspect(project,workspace)
    assert observed['env_probe']=={'mode':'local-smoke','hostname':observed['hostname']}
    report={'project_id':project,'workspace':workspace,'loopback_only':True,'no_host_mounts':True,
            'host_credentials_absent':True,'pause_resume_file_retained':True,'env_in_container':True,
            'proxy_status_preserved':True,'native_agent_verified':False,'container_restore_verified':False}
    if args.restore:
        old=owned_container(project,workspace)
        # Only this run's newly created, independently ownership-checked container.
        local.run(['docker','rm','-f',old['Id']],stdout=subprocess.DEVNULL)
        workspace=cli(home,'workspace','restore',project)['workspace']
        workspace=wait(lambda:cli(home,'workspace','refresh',project)['workspace'],lambda w:w['state']=='running')
        observed=inspect(project,workspace)
        assert workspace['sandbox_id']!=report['workspace']['sandbox_id']
        assert workspace['volume_name']==report['workspace']['volume_name'] and observed['marker']==marker
        report.update(container_restore_verified=True,workspace=workspace)
    if args.agent:
        print('Native company Agent smoke starting (bounded container read/test/write).',file=sys.stderr,flush=True)
        message="""This is an authorized local Docker smoke in /workspace, not an EDC business task.
Use native shell tools in the attached execution environment only; do not call MCP or web tools.
Read repo/marker.txt and repo/calculator.py, run `cd /workspace/repo && python3 -m unittest -v`.
Write /workspace/output/agent-probe.json with JSON keys hostname (socket.gethostname()),
marker (the exact file content), and result (calculator.add(2,3)). Do not change source files.
Do not read credentials, use external services or request elevated permissions. End with a short summary.
"""
        result=cli(home,'send',project,'--message',message,'--wait','--timeout-seconds','180',timeout=195)
        if result['run']['status']!='completed':
            raise RuntimeError('Agent smoke not completed; inspect the existing Run, do not resubmit blindly')
        observed=inspect(project,workspace)
        assert observed['agent_probe']=={'hostname':observed['hostname'],'marker':marker,'result':5}
        assert observed['credentials_absent'] and observed['model_keys_absent']
        report.update(native_agent_verified=True,run_id=result['run']['id'])
    local.write_private(home/'runtime/local-smoke-report.json',json.dumps(report,indent=2),replace=True)
    print(json.dumps(report,indent=2))


if __name__=='__main__':
    main()
