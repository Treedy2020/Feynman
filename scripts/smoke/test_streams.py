"""Regression checks for push transport and durable, non-authorizing attention."""
import json
import unittest
from urllib.request import Request
from urllib.parse import quote
from test_console import OPENER, browser, request
from test_project_manager import platform, cli, wait_for


def stream(base, token, path):
    return OPENER.open(Request(base+'/console/api/v1/'+path,
        headers={'X-Feynman-Console':token, 'Origin':base}),timeout=8)


def event(response):
    value={}
    while True:
        line=response.readline().decode().strip()
        if not line:
            if 'data' in value:
                value['data']=json.loads(value['data']);return value
            continue
        if line.startswith(':'):continue
        key,_,content=line.partition(':')
        value[key]=content.strip()


class StreamTest(unittest.TestCase):
    def test_project_snapshot_stream_reply_and_restart(self):
        with platform() as (base,root,start,stop):
            p=cli(base,'projects','create','--name','SSE regression')['project']['id']
            token=request(base,'GET','/console/session')[2]['csrf_token']
            with stream(base,token,f'projects/{p}/stream') as connection:
                self.assertIn('text/event-stream',connection.headers['Content-Type'])
                self.assertEqual(event(connection)['event'],'snapshot')
                ack=browser(base,token,'POST',f'projects/{p}/inbox',{'content':'STREAM_DELAY inspect only','delivery':'steer_if_active'})
                self.assertEqual(ack[0],200)
                seen=[]
                for _ in range(20):
                    frame=event(connection);seen.append(frame)
                    c=frame['data'].get('conversation')
                    if c and c['runs'] and c['runs'][-1]['status']=='completed':break
                self.assertTrue(any(f['event']=='run_event' and f['data']['event']['kind']=='agent_text_delta' for f in seen),seen)
                final=seen[-1]['data']['conversation']
                self.assertIn('fixture reply',final['messages'][-1]['content'])
            stop();start();token=request(base,'GET','/console/session')[2]['csrf_token']
            with stream(base,token,f'projects/{p}/stream') as reconnected:
                snapshot=event(reconnected)['data']['conversation']
                self.assertEqual(snapshot,final)
            self.assertEqual(request(base,'GET',f'/console/api/v1/projects/{p}/stream')[0],403)

    def test_notification_replay_dedup_read_is_not_approval_and_persistence(self):
        with platform() as (base,root,start,stop):
            p=cli(base,'projects','create','--name','Notification regression')['project']['id']
            token=request(base,'GET','/console/session')[2]['csrf_token']
            with stream(base,token,'notifications/stream') as connection:
                ready=event(connection);self.assertEqual(ready['id'],'0')
                browser(base,token,'POST',f'projects/{p}/inbox',{'content':'BLOCK: approval regression'})
                frame=event(connection);self.assertEqual(frame['event'],'notifications')
                notice=frame['data']['items'][0];self.assertEqual(notice['kind'],'approval_requested')
                self.assertFalse(notice['read']);self.assertFalse(notice['resolved'])
                cursor=frame['id']
                self.assertEqual(browser(base,token,'POST',f'notifications/{notice["id"]}/read',{})[0],204)
                self.assertEqual(cli(base,'status',p)['main_agent']['active_run']['status'],'waiting_approval')
                head=browser(base,token,'GET','notifications?after=0')[2]
                self.assertEqual(head['unread'],0);self.assertTrue(head['items'][0]['read'])
                run=notice['run_id']
                feed=browser(base,token,'GET',f'projects/{p}/runs/{run}/events?after=0')[2]
                approval=next(e for e in feed['events'] if e['kind']=='approval_requested')['payload']['requestId']
                self.assertEqual(browser(base,token,'POST',f'projects/{p}/runs/{run}/approvals/{quote(approval,safe="")}',{'decision':'accept'})[0],204)
                wait_for(lambda:cli(base,'status',p),lambda s:s['main_agent']['active_run'] is None)
            with stream(base,token,'notifications/stream?after='+cursor) as replay:
                self.assertEqual(event(replay)['id'],cursor)
                next_frame=event(replay)
                self.assertEqual([n['kind'] for n in next_frame['data']['items']],['run_completed'])
            self.assertEqual(browser(base,token,'GET','notifications?after=-1')[0],400)
            records=browser(base,token,'GET','notifications')[2]
            self.assertTrue(records['items'][0]['resolved'])
            stop();start();token=request(base,'GET','/console/session')[2]['csrf_token']
            self.assertEqual(browser(base,token,'GET','notifications')[2],records)


if __name__=='__main__':unittest.main()
