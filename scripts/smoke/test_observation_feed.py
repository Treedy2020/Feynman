"""Read-only terminal channels and unified inbox. No real model/DWS calls."""
from contextlib import contextmanager
import json
import queue
import signal
import subprocess
import threading
import time
import unittest

from test_project_manager import platform, cli, create, wait_for, ROOT
from test_automation import new_source, ingest, input_cli, authorize_dws, scope, utc, mcp
from test_external_access import platform as access_platform, call, grant, OPENER
from datetime import timedelta
from urllib.request import Request
from urllib.error import HTTPError


class Watch:
    def __init__(self, base, *args):
        self.process = subprocess.Popen([str(ROOT/'target/debug/feynman'), '--server', base,
                                        '--json', *args], text=True, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE)
        self.events = []
        self.queue = queue.Queue()
        def read():
            for line in self.process.stdout:
                value = json.loads(line)
                self.events.append(value)
                self.queue.put(value)
        self.thread = threading.Thread(target=read, daemon=True)
        self.thread.start()

    def next(self, kind, seconds=12):
        until = time.monotonic()+seconds
        while time.monotonic()<until:
            value = self.queue.get(timeout=max(.01, until-time.monotonic()))
            if value['event']==kind:
                return value
        raise AssertionError(f'no {kind}: {self.events}')

    def close(self):
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGINT)
        self.process.wait(timeout=12)
        self.thread.join(timeout=2)
        self.process.stdout.close()
        self.error = self.process.stderr.read()
        self.process.stderr.close()


@contextmanager
def watch(base, *args):
    view = Watch(base, *args)
    try:
        yield view
    finally:
        view.close()


class ObservationFeedTest(unittest.TestCase):
    def test_merge_pagination_partitioning_and_no_read_side_effects(self):
        with platform() as (base, root, start, stop):
            p=create(base,'Unified feed')['id'];other=create(base,'Other')['id']
            a=new_source(base,root,p);b=new_source(base,root,p);peer=new_source(base,root,other)
            ingest(base,root,p,a['id'],event='same',text='A')
            ingest(base,root,other,peer['id'],event='peer',text='Do not reveal')
            ingest(base,root,p,b['id'],event='same',text='B')
            self.assertEqual(ingest(base,root,p,a['id'],event='same')['inserted'],0)
            first=cli(base,'feed','read',p,'--limit','1')
            self.assertTrue(first['has_more']);self.assertEqual(first['records'][0]['source_id'],a['id'])
            second=cli(base,'feed','read',p,'--after',str(first['next_after']),'--limit','1')
            self.assertFalse(second['has_more']);self.assertEqual(second['records'][0]['body'],{'text':'B'})
            self.assertEqual(len(cli(base,'feed','read',p)['records']),2)
            self.assertEqual(len(cli(base,'feed','read',p,'--source',b['id'])['records']),1)
            self.assertEqual(cli(base,'inbox','list',p)['inbox'],[])
            self.assertFalse((root/'codex.jsonl').exists())
            head=call(base,'GET',f'/v1/projects/{p}/feed?head=true')[1]
            self.assertEqual(head['records'],[]);self.assertEqual(head['next_after'],second['next_after'])
            self.assertNotIn('Do not reveal',json.dumps(head))
            self.assertNotIn('arguments',json.dumps(head['channels']))
            for query in ['limit=201','after=-1','source_id='+peer['id']]:
                self.assertNotEqual(call(base,'GET',f'/v1/projects/{p}/feed?{query}')[0],200)
            req=Request(base+f'/v1/projects/{p}/feed',headers={'Origin':'https://attacker.example'})
            with self.assertRaises(HTTPError) as error: OPENER.open(req)
            self.assertEqual(error.exception.code,401)

    def test_independent_windows_stream_only_new_records_and_do_not_stop_collection(self):
        with platform() as (base,root,start,stop):
            p=create(base,'Views')['id'];a=new_source(base,root,p);b=new_source(base,root,p)
            ingest(base,root,p,a['id'],event='old',text='Before windows')
            with watch(base,'sources','watch',p,a['id']) as va, \
                 watch(base,'sources','watch',p,a['id']) as mirror, \
                 watch(base,'sources','watch',p,b['id']) as vb, \
                 watch(base,'feed','watch',p) as combined:
                for view in [va,mirror,vb,combined]: view.next('channels')
                ingest(base,root,p,b['id'],event='new-b',text='Only B')
                ingest(base,root,p,a['id'],event='new-a',text='Only A')
                for view in [va,mirror]:
                    event=view.next('observation');self.assertEqual(event['record']['body']['text'],'Only A')
                self.assertEqual(vb.next('observation')['record']['body']['text'],'Only B')
                self.assertEqual([combined.next('observation')['record']['body']['text'] for _ in range(2)],['Only B','Only A'])
                self.assertFalse(any(e.get('record',{}).get('event_id')=='old' for v in [va,mirror,vb,combined] for e in v.events))
            self.assertTrue(all(s['enabled'] for s in cli(base,'sources','list',p)['sources']))
            self.assertEqual(cli(base,'inbox','list',p)['inbox'],[])
            self.assertFalse((root/'codex.jsonl').exists())
            self.assertTrue(all(v.process.returncode==0 for v in [va,mirror,vb,combined]))

    def test_replay_reconnect_and_resume_use_persisted_sequence_not_provider_cursor(self):
        with platform() as (base,root,start,stop):
            p=create(base,'Resume')['id'];a=new_source(base,root,p)
            ingest(base,root,p,a['id'],event='A')
            with watch(base,'feed','watch',p,'--after','0') as view:
                first=view.next('observation')['record']['sequence']
                stop();view.next('reconnecting');start()
                ingest(base,root,p,a['id'],event='B')
                second=view.next('observation')['record']['sequence']
                self.assertGreater(second,first)
                self.assertEqual([e['record']['event_id'] for e in view.events if e['event']=='observation'],['A','B'])
            with watch(base,'sources','watch',p,a['id'],'--after',str(first)) as resumed:
                self.assertEqual(resumed.next('observation')['record']['event_id'],'B')
            self.assertEqual(cli(base,'sources','list',p)['sources'][0]['cursor'],a['cursor'])

    def test_agent_feed_is_fixed_to_its_identity_and_retained_history_is_owner_only(self):
        with platform() as (base,root,start,stop):
            p=create(base,'Agent feed')['id'];old=cli(base,'agents','list',p)['agents'][0]['id']
            source=new_source(base,root,p);ingest(base,root,p,source['id'])
            result,tools=mcp(base,root,p,{'operation':'feed'})
            self.assertFalse(result['isError']);self.assertEqual(len(json.loads(result['content'][0]['text'])['records']),1)
            cli(base,'agents','delete',p,old)
            new=cli(base,'agents','create',p,'--name','Replacement','--default')['agent']['id']
            result,_=mcp(base,root,p,{'operation':'feed'},agent=new)
            self.assertFalse(result['isError']);self.assertEqual(json.loads(result['content'][0]['text'])['records'],[])
            result,_=mcp(base,root,p,{'operation':'feed','source_id':source['id']},agent=new)
            self.assertTrue(result['isError'])
            self.assertEqual(len(cli(base,'feed','read',p)['records']),1)
            result,_=mcp(base,root,p,{'operation':'feed'},agent=old)
            self.assertTrue(result['isError'])

    def test_dws_live_channel_outlives_view_and_revocation_filters_before_cursor(self):
        with access_platform() as (base,p,peer,root,env):
            connection=authorize_dws(base,p);grant(base,p,'dws.event.messages.listen')
            now=utc();begin=(now-timedelta(hours=1)).isoformat();end=(now+timedelta(hours=1)).isoformat()
            scope(base,p,begin,end)
            source=input_cli(base,root,'sources','create',p,value={'name':'Live DWS','kind':'dws_events','enabled':True,
                'connection_id':connection,'arguments':{'target_kind':'group','target_id':'cidFixture==','start':begin,'end':end}})['source']['id']
            wait_for(lambda:cli(base,'sources','read',p,source)['records'])
            with watch(base,'sources','watch',p,source,'--after','0') as view:
                self.assertEqual(view.next('observation')['record']['event_id'],'fixture-event')
            self.assertEqual(cli(base,'sources','list',p)['sources'][0]['listener_state'],'live')
            with watch(base,'sources','watch',p,source) as view:
                view.next('channels')
                call(base,'DELETE',f'/v1/projects/{p}/external-tools/dws.chat.messages.read')
                self.assertEqual(view.process.wait(timeout=8),1)
            page=cli(base,'feed','read',p)
            self.assertEqual(page['records'],[]);self.assertEqual(page['channels'],[]);self.assertEqual(page['next_after'],0)
            self.assertEqual(call(base,'GET',f'/v1/projects/{p}/feed?head=true')[1]['next_after'],0)
            self.assertNotEqual(call(base,'GET',f'/v1/projects/{p}/feed?source_id={source}')[0],200)
            self.assertFalse((root/'codex.jsonl').exists())


if __name__=='__main__': unittest.main()
