"""Agent/Source/Schedule v1 public interfaces. Only upstream CLIs are fixtures.

No real authentication, model request, event subscription or external message is sent.
"""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import time
import unittest
import uuid
from contextlib import closing
from datetime import datetime, timedelta, timezone
from urllib.request import Request
from urllib.error import HTTPError

from test_project_manager import platform, cli, create, wait_for, ROOT
from test_external_access import platform as access_platform, call, grant, OPENER


def utc():
    return datetime.now(timezone.utc)


def input_cli(base, root, *args, value):
    path = root / (str(uuid.uuid4()) + '.json')
    path.write_text(json.dumps(value))
    return cli(base, *args, '--input', str(path))


def new_source(base, root, project, **fields):
    return input_cli(base, root, 'sources', 'create', project,
                     value=dict(name='Facts', kind='manual', enabled=True, **fields))['source']


def ingest(base, root, project, source, event='shared-message', text='Fact'):
    return input_cli(base, root, 'sources', 'ingest', project, source,
                     value={'records': [{'event_id': event, 'occurred_at': utc().isoformat(),
                                         'body': {'text': text, 'access_token': 'SECRET_CANARY'}}]})


def mcp(base, root, project, arguments, db=None, agent=None):
    env = dict(os.environ, FEYNMAN_DB=str(db or root/'data/rust-platform.sqlite3'),
               FEYNMAN_AGENT_PROJECT_ID=project, FEYNMAN_AGENT_USER_ID='local-user',
               FEYNMAN_AGENT_TENANT_ID='local', FEYNMAN_ACCESS_SERVER_URL=base)
    if agent:
        env['FEYNMAN_AGENT_ID']=agent
    else:
        env.pop('FEYNMAN_AGENT_ID',None)
    messages = [{'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {}},
                {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list', 'params': {}},
                {'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call',
                 'params': {'name': 'feynman_observations', 'arguments': arguments}}]
    result = subprocess.run([str(ROOT/'target/debug/feynman-server'), 'external-tools-mcp'],
                            input='\n'.join(json.dumps(m) for m in messages)+'\n',
                            env=env, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert 'SECRET_CANARY' not in result.stdout
    values = [json.loads(line) for line in result.stdout.splitlines()]
    return values[-1]['result'], values[1]['result']['tools']


def authorize_dws(base, project):
    grant(base, project, 'dws.auth.device')
    status, data = call(base, 'POST', f'/v1/projects/{project}/dws-authorizations', {'reason':'Offline source test'})
    assert status == 202, (status,data)
    auth = wait_for(lambda: call(base, 'GET', f"/v1/projects/{project}/dws-authorizations/{data['authorization']['id']}")[1]['authorization'],
                    lambda a: a['status']=='authorized')
    status, data = call(base, 'POST', '/v1/access/connections',
                        {'provider':'dws','project_id':project,'authorization_id':auth['id']})
    assert status == 200, data
    connection = data['connection']['id']
    assert call(base, 'POST', f'/v1/projects/{project}/access/bindings',
                {'provider':'dws','connection_id':connection})[0] == 200
    grant(base, project, 'dws.chat.messages.read')
    return connection


def scope(base, project, start, end):
    cli(base,'access','dws-scope',project,'--scope-json',json.dumps({
        'targets':[{'kind':'group','id':'cidFixture=='}], 'start':start,'end':end}))


class AutomationTest(unittest.TestCase):
    def test_registration_partitions_survive_restart_and_are_readable_by_the_agent(self):
        with platform() as (base,root,start,stop):
            project=create(base,'Durable facts')['id']; peer=create(base,'Peer')['id']
            a=new_source(base,root,project)
            ingest(base,root,project,a['id'],text='Earlier registration')
            b=new_source(base,root,project)
            self.assertNotEqual(a['id'],b['id'])
            self.assertEqual(ingest(base,root,project,b['id'],text='Later registration')['inserted'],1)
            self.assertEqual(ingest(base,root,project,a['id'])['inserted'],0)
            rows=cli(base,'sources','read',project,a['id'])['records']
            self.assertEqual(rows[0]['body'],{'text':'Earlier registration'})
            self.assertEqual(cli(base,'sources','read',project,b['id'])['records'][0]['body'],{'text':'Later registration'})
            self.assertEqual(cli(base,'sources','read',project,a['id'],'--after',str(rows[0]['sequence']))['records'],[])
            cli(base,'sources','read',peer,a['id'],ok=False)
            cli(base,'sources','read',project,a['id'],'--limit','201',ok=False)
            result,tools=mcp(base,root,project,{'operation':'query','source_id':a['id']})
            self.assertFalse(result['isError'])
            self.assertEqual(json.loads(result['content'][0]['text'])['records'],rows)
            definition=next(t for t in tools if t['name']=='feynman_observations')
            self.assertTrue(definition['annotations']['readOnlyHint'])
            denied,_=mcp(base,root,peer,{'operation':'query','source_id':a['id']})
            self.assertTrue(denied['isError'])
            denied,_=mcp(base,root,project,{'operation':'ingest','source_id':a['id']})
            self.assertTrue(denied['isError'])
            stop();start()
            self.assertEqual(cli(base,'sources','read',project,a['id'])['records'],rows)
            cli(base,'sources','pause',project,a['id'])
            self.assertEqual(cli(base,'sources','read',project,a['id'])['records'],rows)

    def test_queue_and_native_steer_use_the_existing_turn_without_double_start(self):
        with platform() as (base,root,start,stop):
            project=create(base,'Steer')['id']
            run=cli(base,'send',project,'--message','HOLD')['run']
            queued=cli(base,'inbox','send',project,'--message','Queued work')['message']
            time.sleep(1.2)
            self.assertEqual(cli(base,'inbox','list',project)['inbox'][0]['state'],'pending')
            # FIFO: cancel this pending queue entry explicitly before the steer entry.
            cli(base,'inbox','resolve',project,queued['id'],'--state','cancelled')
            steering=cli(base,'inbox','send',project,'--message','FINISH','--delivery','steer_if_active')['message']
            messages=wait_for(lambda: cli(base,'inbox','list',project)['inbox'],
                              lambda items:any(m['id']==steering['id'] and m['state']=='accepted' for m in items))
            self.assertEqual(next(m for m in messages if m['id']==steering['id'])['run_id'],run['id'])
            calls=[json.loads(line) for line in (root/'codex.jsonl').read_text().splitlines()]
            self.assertEqual(sum(c.get('method')=='turn/start' for c in calls),1)
            steer=next(c for c in calls if c.get('method')=='turn/steer')
            self.assertEqual(steer['params']['expectedTurnId'],cli(base,'runs',project,run['id'])['run']['runtime_turn_id'])
            self.assertIn(steering['id'],steer['params']['input'][0]['text'])

    def test_whole_schedule_window_stays_fixed_across_sources_and_restart(self):
        with platform() as (base,root,start,stop):
            project=create(base,'Fixed window')['id']
            a=new_source(base,root,project);b=new_source(base,root,project)
            plan=cli(base,'schedules','create',project,'--title','Listen','--prompt','Inspect changes',
                     '--source',a['id'],'--source',b['id'],'--collect-for-seconds','2')['schedule']
            cli(base,'schedules','resume',project,plan['id'])
            ingest(base,root,project,a['id'],event='A')
            # Wait until the opening timestamp exists, then restart inside the window.
            def window():
                with closing(sqlite3.connect(root/'data/rust-platform.sqlite3')) as c:
                    return c.execute('SELECT opened,closes FROM schedule_windows WHERE schedule=?',(plan['id'],)).fetchone()
            opened=wait_for(window)
            ingest(base,root,project,b['id'],event='B')
            stop();start()
            report=wait_for(lambda:cli(base,'schedules','deliveries',project,plan['id']),lambda r:len(r['occurrences'])==1 and r['inbox'][0]['state']=='accepted')
            occurrence=report['occurrences'][0]
            self.assertEqual(occurrence['opened_at'],opened[0]);self.assertEqual(occurrence['closed_at'],opened[1])
            self.assertEqual(occurrence['end_sequence']-occurrence['start_sequence'],1)
            stop();start();time.sleep(1.2)
            self.assertEqual(len(cli(base,'schedules','deliveries',project,plan['id'])['occurrences']),1)

    def test_unknown_is_never_automatically_replayed_and_owner_can_reconcile(self):
        with platform() as (base,root,start,stop):
            project=create(base,'Unknown delivery')['id']
            cli(base,'send',project,'--message','HOLD')
            message=cli(base,'inbox','send',project,'--message','Uncertain input')['message']
            stop()
            # Simulate a crash between dispatch commit and upstream confirmation.
            with closing(sqlite3.connect(root/'data/rust-platform.sqlite3')) as c,c:
                message['state']='dispatching'
                c.execute('UPDATE agent_inbox SET state=?,data=? WHERE id=?',('dispatching',json.dumps(message),message['id']))
            start();time.sleep(1.2)
            self.assertEqual(cli(base,'inbox','list',project)['inbox'][0]['state'],'unknown')
            before=(root/'codex.jsonl').read_text()
            self.assertNotIn('Uncertain input',before)
            cli(base,'inbox','resolve',project,message['id'],'--state','pending')
            wait_for(lambda:cli(base,'inbox','list',project)['inbox'],lambda items:items[0]['state']=='accepted')
            calls=[json.loads(line) for line in (root/'codex.jsonl').read_text().splitlines()]
            self.assertEqual(sum(c.get('method')=='turn/start' and 'Uncertain input' in str(c) for c in calls),1)

    def test_owner_control_no_default_replacement_and_browser_origin_rejection(self):
        with platform() as (base,root,start,stop):
            empty=cli(base,'projects','create','--name','Workspace first','--without-agent')['project']['id']
            self.assertEqual(cli(base,'agents','list',empty)['agents'],[])
            self.assertIsNone(cli(base,'status',empty)['main_agent'])
            cli(base,'inbox','send',empty,'--message','No Agent',ok=False)
            self.assertEqual(cli(base,'agents','create',empty,'--name','Explicit','--default')['agent']['capabilities'],[])
            project=create(base,'Agent lifecycle')['id']
            agent=cli(base,'agents','list',project)['agents'][0]
            old=new_source(base,root,project);ingest(base,root,project,old['id'])
            cli(base,'agents','disable',project,agent['id'])
            cli(base,'inbox','send',project,'--message','Disabled',ok=False)
            cli(base,'agents','enable',project,agent['id'])
            plan=cli(base,'schedules','create',project,'--title','Old Agent','--prompt','Inspect','--at','2026-01-01T00:00:00Z')['schedule']
            cli(base,'agents','delete',project,agent['id'])
            replacement=cli(base,'agents','create',project,'--name','Replacement')['agent']
            self.assertEqual(replacement['capabilities'],[])
            cli(base,'schedules','resume',project,plan['id'],ok=False)
            cli(base,'inbox','send',project,'--message','No default',ok=False)
            sent=cli(base,'inbox','send',project,'--agent',replacement['id'],'--message','Explicit route')['message']
            wait_for(lambda:cli(base,'inbox','list',project)['inbox'],lambda items:any(m['id']==sent['id'] and m['state']=='accepted' for m in items))
            result,_=mcp(base,root,project,{'operation':'query','source_id':old['id']})
            self.assertTrue(result['isError'])
            stale,_=mcp(base,root,project,{'operation':'sources'},agent=agent['id'])
            self.assertTrue(stale['isError'],'lazy MCP must keep the original Agent identity')
            # Owner may inspect retained manual evidence, replacement Agent may not.
            self.assertEqual(len(cli(base,'sources','read',project,old['id'])['records']),1)
            req=Request(base+f'/v1/projects/{project}/agents',method='POST',data=b'{"name":"Browser"}',headers={'Content-Type':'application/json','Origin':'https://attacker.example'})
            with self.assertRaises(HTTPError) as error: OPENER.open(req)
            self.assertEqual(error.exception.code,401)

    def test_dws_backfill_live_filtering_capture_ledger_and_revocation(self):
        with access_platform() as (base,project,peer,root,env):
            connection=authorize_dws(base,project)
            now=utc();start=(now-timedelta(hours=2)).isoformat();end=(now+timedelta(hours=2)).isoformat()
            scope(base,project,start,end)
            args={'target_kind':'group','target_id':'cidFixture==','start':start,'end':end}
            data={'name':'DWS facts','kind':'dws_events','enabled':False,'connection_id':connection,'arguments':args}
            self.assertEqual(call(base,'POST',f'/v1/projects/{project}/sources',data)[0],403)
            grant(base,project,'dws.event.messages.listen')
            status,response=call(base,'POST',f'/v1/projects/{project}/sources',data)
            self.assertEqual(status,200,response)
            source=response['source']['id']
            # Enable explicitly: a new listener uses a stable target/profile, not arbitrary argv.
            cli(base,'sources','resume',project,source)
            records=wait_for(lambda:cli(base,'sources','read',project,source)['records'])
            self.assertEqual(len(records),1)
            self.assertEqual(records[0]['body']['conversation_id'],'cidFixture==')
            self.assertNotIn('access_token',records[0]['body'])
            self.assertEqual(records[0]['event_id'],'fixture-event')
            report=cli(base,'sources','backfill',project,source,'--start',start,'--end',(now-timedelta(hours=1)).isoformat())
            self.assertTrue(report['complete'])
            self.assertEqual(report['cursor'],start.replace('+00:00','Z'))
            captures=cli(base,'sources','captures',project,source)['captures']
            self.assertEqual(captures[0]['kind'],'backfill')
            result,_=mcp(base,root,project,{'operation':'query','source_id':source})
            self.assertFalse(result['isError'])
            agent=cli(base,'agents','list',project)['agents'][0]
            cli(base,'send',project,'--message','HOLD')
            revoked=cli(base,'agents','revoke',project,agent['id'],'dws.chat.messages.read')['agent']
            self.assertNotIn('dws.chat.messages.read',revoked['capabilities'])
            # Immediate revocation while a model Run is active also stops the owned listener.
            wait_for(lambda:cli(base,'sources','list',project)['sources'][0],lambda s:s['listener_state']=='stopped')
            self.assertEqual(call(base,'GET',f'/v1/projects/{project}/sources/{source}/records')[0],403)
            result,_=mcp(base,root,project,{'operation':'query','source_id':source})
            self.assertTrue(result['isError'])
            self.assertTrue(list((root/'data/private/dws').rglob('listener-cleaned')))
            public=json.dumps(cli(base,'sources','list',project))
            self.assertNotIn('FIXTURE_SECRET_MUST_NOT_LEAK',public)

    def test_partial_backfill_retains_cursor_and_binding_cannot_switch_a_source_identity(self):
        with access_platform() as (base,project,peer,root,env):
            connection=authorize_dws(base,project)
            start='2026-09-01T00:00:00+08:00';end='2026-09-02T00:00:00+08:00'
            scope(base,project,start,end)
            data={'name':'Bound history','kind':'dws_messages','enabled':True,'connection_id':connection,
                  'arguments':{'target_kind':'group','target_id':'cidFixture==','start':start,'end':end}}
            # Use the same fixture executable path; no credential/cache/identity change.
            fixture=root/'success'
            original=fixture.read_text()
            fixture.write_text(original.replace("mode = Path(sys.argv[0]).name","mode = 'partial'"))
            status,response=call(base,'POST',f'/v1/projects/{project}/sources',data)
            self.assertEqual(status,200,response);source=response['source']
            report=cli(base,'sources','backfill',project,source['id'],'--start',start,'--end',end)
            self.assertFalse(report['complete'])
            self.assertEqual(report['cursor'],source['cursor'])
            captures=cli(base,'sources','captures',project,source['id'])['captures']
            self.assertTrue(any(not capture['complete'] for capture in captures))
            self.assertEqual(len(cli(base,'sources','read',project,source['id'])['records']),1)
            cli(base,'sources','pause',project,source['id'])
            fixture.write_text(original)
            newer=authorize_dws(base,project)
            self.assertNotEqual(newer,connection)
            # New project binding must NOT silently change this Source's immutable connection.
            cli(base,'sources','resume',project,source['id'],ok=False)
            result,_=mcp(base,root,project,{'operation':'query','source_id':source['id']})
            self.assertTrue(result['isError'])
            self.assertEqual(call(base,'PUT',f"/v1/projects/{project}/sources/{source['id']}",
                                  {'enabled':True,'connection_id':newer})[0],422)


if __name__=='__main__':
    unittest.main()
