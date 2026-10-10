"""Public CLI/MCP + real Unix workload lifecycle; upstream model is an offline fixture."""
from contextlib import contextmanager, closing
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import time
import unittest
from unittest.mock import patch
import uuid
import tempfile
import shutil

from test_project_manager import platform, cli, create, wait_for, ROOT
from test_external_access import call
from test_automation import input_cli, new_source, ingest, authorize_dws, scope, utc
from test_external_access import platform as access_platform
from datetime import timedelta
from fixtures.opensandbox_lifecycle import upstream

CANARY='REGISTERED_SECRET_CANARY_943902'


@contextmanager
def resources_platform():
    with patch.dict(os.environ, {'FEYNMAN_SECRET_STORE':'private_file'}):
        with platform() as values:
            yield values


def rpc(base,project,operation,arguments=None,agent=None):
    route=f'/v1/projects/{project}/resources' if not agent else f'/v1/projects/{project}/agent-resources/{agent}'
    return call(base,'POST',route,{'operation':operation,'arguments':arguments or {}})


def success(base,project,operation,arguments=None,agent=None):
    status,data=rpc(base,project,operation,arguments,agent)
    assert status==200,(status,data)
    assert CANARY not in json.dumps(data)
    return data


def mcp(base,root,project,agent,name,operation,arguments=None):
    env=dict(os.environ,FEYNMAN_DB=str(root/'data/rust-platform.sqlite3'),FEYNMAN_AGENT_TENANT_ID='local',
             FEYNMAN_AGENT_USER_ID='local-user',FEYNMAN_AGENT_PROJECT_ID=project,
             FEYNMAN_AGENT_ID=agent,FEYNMAN_ACCESS_SERVER_URL=base)
    messages=[{'jsonrpc':'2.0','id':1,'method':'initialize'},
              {'jsonrpc':'2.0','id':2,'method':'tools/call','params':{'name':name,
               'arguments':{'operation':operation,'arguments':arguments or {}}}}]
    result=subprocess.run([str(ROOT/'target/debug/feynman-server'),'external-tools-mcp'],env=env,
                          input='\n'.join(json.dumps(m) for m in messages)+'\n',text=True,capture_output=True,timeout=25)
    assert result.returncode==0,result.stderr
    assert CANARY not in result.stdout
    response=json.loads(result.stdout.splitlines()[-1])['result']
    return response


def secret(base,project,value=CANARY,**kw):
    result=subprocess.run([str(ROOT/'target/debug/feynman'),'--server',base,'secrets','put',project,
                           '--name','test-secret','--stdin',*(['--id',kw['id'],'--revision',str(kw['revision'])] if kw else [])],
                          input=value,text=True,capture_output=True,timeout=10)
    assert result.returncode==0,result.stderr
    assert value not in result.stdout
    return json.loads(result.stdout)['secret']


def profile(base,project,argv,seconds=30,secrets=None,files=None,name='debug'):
    return success(base,project,'profile.put',{'profile':{'name':name,'argv':argv,'variables':{'APP_MODE':'test'},
                'secrets':secrets or {},'secret_files':files or {},'max_seconds':seconds}})['profile']


def approved(base,project,name='debug',seconds=30,auto=False):
    request=success(base,project,'env.request',{'profile':name,'reason':'Offline test','seconds':seconds})['request']
    return success(base,project,'env.approve',{'id':request['id'],'expected_revision':request['revision'],
                'accept_secret_exposure':True,'auto_renew':auto,'seconds':120})['request']


def run(base,project,request,key=None,agent=None):
    return success(base,project,'env.run',{'request_id':request['id'],'idempotency_key':key or str(uuid.uuid4())},agent)['lease']


def lease(base,project,identifier):
    return next(l for l in cli(base,'leases','list',project)['items'] if l['id']==identifier)


def memory(title='Trial requirement',**kw):
    value=dict(title=title,body='Need reproducible formula evaluation',kind='inference');value.update(kw);return value


class ResourcesTest(unittest.TestCase):
    def test_local_opensandbox_workload_uses_docker_without_ssh_or_host_fallback(self):
        with tempfile.TemporaryDirectory(prefix='feynman-local-docker-protocol-') as directory:
            transport=Path(directory);(transport/'workspace').mkdir();(transport/'bin').mkdir()
            docker=transport/'bin/docker'
            fixture=(Path(__file__).parent/'fixtures/workload_docker.py').read_text()
            docker.write_text(fixture.replace("Path(os.environ['FEYNMAN_TEST_WORKLOAD_TRANSPORT'])",'Path('+repr(directory)+')'))
            docker.chmod(0o700)
            ssh=transport/'bin/ssh';ssh.write_text('#!/bin/sh\nexit 99\n');ssh.chmod(0o700)
            with patch.dict(os.environ,{'PATH':str(transport/'bin')+os.pathsep+os.environ['PATH']}):
                with upstream() as (url,state),platform(url) as (base,root,start,stop):
                    project=create(base,'Local Docker workspace')['id']
                    workspace=cli(base,'workspace','create',project,'--provider','opensandbox')['workspace']
                    (transport/'owned.json').write_text(json.dumps({'volume':workspace['volume_name'],'labels':{
                        'feynman-project':project,'feynman-tenant':'local','feynman-environment':workspace['environment_id']}}))
                    profile(base,project,['python3','-c',"import os;from pathlib import Path;Path('local-docker.txt').write_text(os.environ['APP_MODE'])"])
                    request=approved(base,project);result=run(base,project,request)
                    wait_for(lambda:lease(base,project,result['id']),lambda l:l['data']['state']=='completed')
                    self.assertEqual((transport/'workspace/local-docker.txt').read_text(),'test')
                    self.assertFalse(any(root.rglob('local-docker.txt')))
                    self.assertIn('exec',(transport/'docker-argv.jsonl').read_text())

    def test_remote_docker_stdin_protocol_and_verified_tmpfs_fail_closed(self):
        with tempfile.TemporaryDirectory(prefix='feynman-workload-transport-') as directory:
            root=Path(directory);(root/'workspace').mkdir();(root/'bin').mkdir()
            docker=root/'bin/docker';shutil.copyfile(Path(__file__).parent/'fixtures/workload_docker.py',docker);docker.chmod(0o700)
            env=dict(os.environ,PATH=str(root/'bin')+os.pathsep+os.environ['PATH'],FEYNMAN_TEST_WORKLOAD_TRANSPORT=directory)
            runner=(ROOT/'crates/feynman-server/src/resources/runner.py').read_text()
            remote=(ROOT/'crates/feynman-server/src/resources/remote.py').read_text()
            identifier='work_'+uuid.uuid4().hex
            payload={'id':identifier,'instance':str(uuid.uuid4()),'root':'/tmp/feynman-workloads','workspace':'/workspace',
                     '_runner_code':runner,'expires_at':int(time.time()*1000)+5000,
                     'argv':[sys.executable,'-c',"import os,time;print(os.environ['API_TOKEN'],flush=True);time.sleep(60)"],
                     'env':{'API_TOKEN':CANARY},'secret_values':[CANARY],'files':{},'cwd':''}
            def control(action,volume='fixture-volume',project='fixture-project'):
                result=subprocess.run([sys.executable,'-c',remote],input=json.dumps({'sandbox_id':payload['instance'],
                     'volume':volume,'feynman-project':project,'feynman-tenant':'local','feynman-environment':'fixture-environment',
                     'code':runner,'action':action,'payload':payload}).encode(),env=env,capture_output=True,timeout=20)
                self.assertNotIn(CANARY.encode(),result.stdout+result.stderr)
                return result
            self.assertEqual(control('start','wrong-volume').returncode,1)
            self.assertEqual(control('start',project='wrong-project').returncode,1)
            self.assertEqual(control('start').returncode,0)
            try:
                logs=wait_for(lambda:json.loads(control('logs').stdout),lambda s:'[REDACTED]' in s.get('output',''))
                self.assertEqual(logs['state'],'running')
                self.assertNotIn(CANARY,(root/'docker-argv.jsonl').read_text())
            finally:
                self.assertEqual(control('stop').returncode,0)
            self.assertEqual(json.loads(control('status').stdout)['state'],'stopped')
            payload.update(id='work_'+uuid.uuid4().hex,expires_at=int(time.time()*1000)+5000,env={},files={'PASSWORD_FILE':CANARY},
                           argv=[sys.executable,'-c',"import os;from pathlib import Path;print(Path(os.environ['PASSWORD_FILE']).read_text())"])
            result=json.loads(control('start').stdout)
            if sys.platform!='linux':
                self.assertEqual(result['state'],'failed')
            else:
                result=wait_for(lambda:json.loads(control('status').stdout),lambda s:s['state'] in ('completed','failed'))
                self.assertIn(result['state'],('completed','failed'))
            self.assertFalse((root/'workspace'/'.env').exists())

    def test_auto_renew_preserves_instance_and_restart_only_reconciles(self):
        with resources_platform() as (base,root,start,stop):
            p=create(base,'Renewal')['id'];w=cli(base,'workspace','create',p,'--provider','local')['workspace']
            profile(base,p,[sys.executable,'-c',"import time;from pathlib import Path;Path('once').write_text('one');time.sleep(60)"],seconds=8)
            request=approved(base,p,seconds=8,auto=True);l=run(base,p,request)
            old=l['data']['expires_at']
            current=wait_for(lambda:lease(base,p,l['id']),lambda l:l['data']['expires_at']>old,seconds=12)
            self.assertEqual(current['data']['sandbox_instance_id'],l['data']['sandbox_instance_id'])
            stop();start()
            self.assertEqual(lease(base,p,l['id'])['data']['state'],'active')
            self.assertEqual(len(cli(base,'leases','list',p)['items']),1)
            current=lease(base,p,l['id']);cli(base,'leases','revoke',p,l['id'],'--revision',str(current['revision']))
            self.assertEqual((Path(w['cwd'])/'once').read_text(),'one')

    def test_remote_missing_control_channel_is_unknown_never_local_fallback(self):
        with upstream() as (url,state),platform(url) as (base,root,start,stop):
            p=create(base,'Remote uncertainty')['id']
            cli(base,'workspace','create',p,'--provider','opensandbox')
            profile(base,p,['touch','must-not-run-on-host'])
            request=approved(base,p);key=str(uuid.uuid4());l=run(base,p,request,key)
            self.assertEqual(l['data']['state'],'unknown')
            self.assertFalse(any(root.rglob('must-not-run-on-host')))
            self.assertEqual(run(base,p,request,key)['id'],l['id'])
            self.assertEqual(len(cli(base,'leases','list',p)['items']),1)

            self.assertEqual(rpc(base,p,'env.run',{'request_id':request['id'],'idempotency_key':str(uuid.uuid4())})[0],409)

    def test_stdin_secret_process_only_delivery_redaction_and_idempotency(self):
        with resources_platform() as (base,root,start,stop):
            p=create(base,'Environment')['id'];other=create(base,'Other')['id']
            s=secret(base,p)
            w=cli(base,'workspace','create',p,'--provider','local')['workspace']
            # Application sees the credential, unrelated native shell and container configuration do not.
            code="import os,time;from pathlib import Path;s=os.environ['API_TOKEN'];print(s[:12],end='',flush=True);time.sleep(.2);print(s[12:],flush=True);print(os.environ['APP_MODE'],flush=True);Path('executed.txt').write_text('once');time.sleep(30)"
            profile(base,p,[sys.executable,'-c',code],secrets={'API_TOKEN':s['id']})
            a=cli(base,'agents','list',p)['agents'][0]['id']
            denied=mcp(base,root,p,a,'feynman_environment','approve',{})
            self.assertTrue(denied['isError'])
            request=success(base,p,'env.request',{'profile':'debug','reason':'Run test'},a)['request']
            self.assertEqual(rpc(base,p,'env.run',{'request_id':request['id'],'idempotency_key':str(uuid.uuid4())},a)[0],403)
            self.assertEqual(rpc(base,p,'env.approve',{'id':request['id'],'expected_revision':1})[0],400)
            request=success(base,p,'env.approve',{'id':request['id'],'expected_revision':1,'accept_secret_exposure':True})['request']
            key=str(uuid.uuid4())
            result=mcp(base,root,p,a,'feynman_environment','run',{'request_id':request['id'],'idempotency_key':key})
            self.assertFalse(result['isError'],result)
            l=json.loads(result['content'][0]['text'])['lease']
            self.assertEqual(run(base,p,request,key)['id'],l['id'])
            self.assertEqual(len(cli(base,'leases','list',p)['items']),1)
            wait_for(lambda:(Path(w['cwd'])/'executed.txt').exists())
            output=wait_for(lambda:cli(base,'env','logs',p,l['id'])['output'],lambda s:'[REDACTED]' in s)
            self.assertIn('test',output)
            self.assertNotIn(CANARY,output)
            self.assertFalse((Path(w['cwd'])/'.env').exists())
            self.assertEqual(rpc(base,other,'env.run',{'request_id':request['id'],'idempotency_key':key})[0],404)
            current=lease(base,p,l['id']);cli(base,'leases','revoke',p,l['id'],'--revision',str(current['revision']))
            wait_for(lambda:lease(base,p,l['id']),lambda l:l['data']['state']=='revoked')
            self.assertEqual(rpc(base,p,'env.run',{'request_id':request['id'],'idempotency_key':str(uuid.uuid4())})[0],403)
            for file in [root/'data/rust-platform.sqlite3',root/'server.log']:
                self.assertNotIn(CANARY.encode(),file.read_bytes())

    def test_expiry_is_enforced_without_server_and_restart_does_not_reissue(self):
        with resources_platform() as (base,root,start,stop):
            p=create(base,'Offline expiry')['id']
            w=cli(base,'workspace','create',p,'--provider','local')['workspace']
            profile(base,p,[sys.executable,'-c',"from pathlib import Path;import time;Path('started').write_text('one');time.sleep(60)"],seconds=3)
            request=approved(base,p,seconds=3);l=run(base,p,request)
            wait_for(lambda:(Path(w['cwd'])/'started').exists())
            stop()
            status=root/'data/private/resources/workloads'/l['id']/'status.json'
            wait_for(lambda:json.loads(status.read_text()),lambda s:s['state']=='expired')
            start()
            current=wait_for(lambda:lease(base,p,l['id']),lambda l:l['data']['state']=='expired')
            self.assertEqual(len(cli(base,'leases','list',p)['items']),1)
            self.assertEqual(current['data']['provider_revocation'],'unsupported')
            cli(base,'leases','renew',p,l['id'],'--revision',str(current['revision']),ok=False)

    def test_rotation_and_agent_disable_revoke_existing_use(self):
        with resources_platform() as (base,root,start,stop):
            p=create(base,'Rotation')['id'];s=secret(base,p)
            profile(base,p,[sys.executable,'-c','import time;time.sleep(60)'],secrets={'API_TOKEN':s['id']})
            request=approved(base,p);l=run(base,p,request)
            secret(base,p,value=CANARY+'new',id=s['id'],revision=s['revision'])
            wait_for(lambda:lease(base,p,l['id']),lambda l:l['data']['state']=='revoked')
            self.assertEqual(rpc(base,p,'env.run',{'request_id':request['id'],'idempotency_key':str(uuid.uuid4())})[0],409)
            request=approved(base,p);l=run(base,p,request)
            a=cli(base,'agents','list',p)['agents'][0]['id'];cli(base,'agents','disable',p,a)
            wait_for(lambda:lease(base,p,l['id']),lambda l:l['data']['state']=='revoked')

    def test_memory_versioning_scope_confirmation_bootstrap_and_private_replacement(self):
        with resources_platform() as (base,root,start,stop):
            p=create(base,'Memory')['id'];other=create(base,'Other')['id'];a=cli(base,'agents','list',p)['agents'][0]['id']
            result=mcp(base,root,p,a,'feynman_memory','put',{'content':memory()})
            self.assertFalse(result['isError'],result);item=json.loads(result['content'][0]['text'])['memory']
            denied=mcp(base,root,p,a,'feynman_memory','put',{'content':memory(scope='project')});self.assertTrue(denied['isError'])
            updated=success(base,p,'memory.put',{'id':item['id'],'expected_revision':1,'content':dict(item['content'],body='Updated')})
            self.assertEqual(updated['memory']['revision'],2)
            self.assertEqual(rpc(base,p,'memory.put',{'id':item['id'],'expected_revision':1,'content':item['content']})[0],409)
            self.assertEqual(rpc(base,other,'memory.get',{'id':item['id']})[0],404)
            cli(base,'memory','confirm',p,item['id'],'--revision','2','--pin')
            shared=success(base,p,'memory.put',{'content':memory(title='Project decision',scope='project',body='Shared architecture')})['memory']
            stop();start()
            self.assertEqual(len(cli(base,'memory','history',p,item['id'])['history']),3)
            cli(base,'send',p,'--message','Check memory','--wait')
            calls=[json.loads(l) for l in (root/'codex.jsonl').read_text().splitlines()]
            self.assertTrue(any('Feynman memory snapshot' in str(c) and 'Updated' in str(c) for c in calls if c.get('method')=='turn/start'))
            cli(base,'agents','delete',p,a)
            new=cli(base,'agents','create',p,'--name','replacement','--default')['agent']['id']
            result=mcp(base,root,p,new,'feynman_memory','search',{})
            ids=[m['id'] for m in json.loads(result['content'][0]['text'])['memories']]
            self.assertNotIn(item['id'],ids);self.assertIn(shared['id'],ids)
            self.assertTrue(mcp(base,root,p,a,'feynman_memory','search',{})['isError'])

    def test_dws_revocation_propagates_through_memory_dependencies_and_search(self):
        with access_platform() as (base,p,peer,root,env):
            connection=authorize_dws(base,p)
            now=utc();start=(now-timedelta(hours=1)).isoformat();end=(now+timedelta(hours=1)).isoformat();scope(base,p,start,end)
            source=input_cli(base,root,'sources','create',p,value={'name':'Historical evidence','kind':'dws_messages','connection_id':connection,'enabled':True,'start':start,'arguments':{'target_kind':'group','target_id':'cidFixture==','start':start,'end':end}})['source']
            records=wait_for(lambda:cli(base,'sources','read',p,source['id'])['records'])
            a=cli(base,'agents','list',p)['agents'][0]['id']
            evidence={'source_id':source['id'],'observation_sequence':records[0]['sequence']}
            item=success(base,p,'memory.put',{'content':memory(evidence=[evidence])})['memory']
            parent=success(base,p,'memory.put',{'content':memory(title='Derived summary',evidence=[{'memory_id':item['id'],'memory_revision':1}])})['memory']
            self.assertEqual(len(success(base,p,'memory.search',agent=a)['memories']),2)
            call(base,'DELETE',f'/v1/projects/{p}/external-tools/dws.chat.messages.read')
            self.assertEqual(success(base,p,'memory.search',agent=a)['memories'],[])
            self.assertNotEqual(rpc(base,p,'memory.get',{'id':parent['id']},a)[0],200)

    def test_protected_env_and_secret_memory_writes_are_rejected(self):
        with resources_platform() as (base,root,start,stop):
            p=create(base,'Validation')['id'];s=secret(base,p)
            self.assertEqual(rpc(base,p,'profile.put',{'profile':{'name':'bad','argv':['true'],'variables':{'PATH':'/tmp'}}})[0],400)
            self.assertEqual(rpc(base,p,'profile.put',{'profile':{'name':'bad','argv':['true'],'cwd':'../outside'}})[0],400)
            self.assertEqual(rpc(base,p,'memory.put',{'content':dict(memory(),body=CANARY)})[0],400)
            self.assertEqual(rpc(base,p,'profile.put',{'profile':{'name':'bad','argv':['true',CANARY]}})[0],400)
            cli(base,'memory','search',p,'--query','试验')
            self.assertEqual(rpc(base,p,'env.approve',{'id':'none','expected_revision':0},create(base,'No access')['main_agent_id'])[0],403)

    def test_secret_registered_later_hides_existing_memory_and_history(self):
        with resources_platform() as (base,root,start,stop):
            p=create(base,'Retroactive secret')['id']
            value='LATER_REGISTERED_CREDENTIAL_88421'
            item=success(base,p,'memory.put',{'content':memory(body=value)})['memory']
            secret(base,p,value=value)
            self.assertEqual(success(base,p,'memory.search')['memories'],[])
            self.assertNotEqual(rpc(base,p,'memory.get',{'id':item['id']})[0],200)
            self.assertEqual(success(base,p,'memory.history',{'id':item['id']})['history'],[])
            deleted=success(base,p,'memory.delete',{'id':item['id'],'expected_revision':1})
            self.assertNotIn(value,json.dumps(deleted))
            self.assertEqual(success(base,p,'memory.history',{'id':item['id']})['history'],[])


if __name__=='__main__':
    unittest.main()
