"""Same Rust owner state through CLI and the protected browser adapter."""
import json
import unittest
from urllib.error import HTTPError
from urllib.request import Request, build_opener, ProxyHandler
from urllib.parse import quote
from test_project_manager import platform, cli, wait_for

OPENER = build_opener(ProxyHandler({}))


def request(base, method, path, body=None, headers=None):
    h = dict(headers or {})
    if body is not None:
        h['Content-Type'] = 'application/json'
    r = Request(base + path, method=method,
                data=None if body is None else json.dumps(body).encode(), headers=h)
    try:
        response = OPENER.open(r, timeout=10)
    except HTTPError as error:
        response = error
    with response:
        raw = response.read()
        return response.status, response.headers, json.loads(raw) if 'application/json' in response.headers.get('Content-Type', '') and raw else raw.decode()


def browser(base, token, method, path, body=None):
    return request(base, method, '/console/api/v1/' + path, body,
                   {'Origin':base, 'X-Feynman-Console':token, 'Sec-Fetch-Site':'same-origin'})


class ConsoleTest(unittest.TestCase):
    def test_system_catalog_is_read_only_and_declares_actual_source_adapters(self):
        with platform() as (base,root,start,stop):
            token = request(base, 'GET', '/console/session')[2]['csrf_token']
            catalog = browser(base,token,'GET','access/providers')
            self.assertEqual(catalog[0],200,catalog)
            providers = {p['provider']:p for p in catalog[2]['providers']}
            self.assertEqual(set(providers),{'dws','gitlab','git','slate','drlee'})
            self.assertEqual(providers['dws']['source_kinds'],['dws_messages','dws_events'])
            for key,provider in providers.items():
                self.assertTrue(provider['display_name'])
                self.assertTrue(provider['summary'])
                self.assertTrue(provider['capabilities'])
                if key != 'dws':self.assertEqual(provider['source_kinds'],[])
            self.assertEqual(browser(base,token,'GET','access/connections')[2]['connections'],[])
            self.assertEqual(cli(base,'projects','list')['projects'],[])
            self.assertFalse((root/'codex.jsonl').exists(),'Catalog must not launch Codex')
            p = cli(base,'projects','create','--name','Provider grouping')['project']['id']
            tools = browser(base,token,'GET',f'projects/{p}/external-tools')[2]['tools']
            actual = {t['id'] for t in tools}
            for provider in providers.values():
                self.assertTrue(set(provider['capabilities']).issubset(actual))
            self.assertFalse(any(t['granted'] for t in tools),'Catalog must not grant project tools')
            self.assertEqual(browser(base,token,'GET',f'projects/{p}/sources')[2]['sources'],[])

    def test_origin_csrf_host_csp_and_old_cli_contract(self):
        with platform() as (base, root, start, stop):
            status, headers, page = request(base, 'GET', '/console/')
            self.assertEqual(status, 200)
            self.assertIn('Feynman', page)
            self.assertEqual(headers['Cache-Control'], 'no-store')
            self.assertIn("frame-ancestors 'none'", headers['Content-Security-Policy'])
            self.assertEqual(headers['X-Content-Type-Options'], 'nosniff')
            self.assertEqual(request(base, 'GET', '/console/', headers={'Host':'evil.test'})[0], 403)
            self.assertEqual(request(base, 'GET', '/console/session', headers={'Sec-Fetch-Site':'cross-site'})[0], 403)
            token = request(base, 'GET', '/console/session')[2]['csrf_token']
            self.assertEqual(request(base, 'POST', '/console/api/v1/projects', {'name':'bad'}, {'Origin':'https://evil.test','X-Feynman-Console':token})[0], 403)
            self.assertEqual(request(base, 'POST', '/console/api/v1/projects', {'name':'bad'}, {'Origin':base})[0], 403)
            self.assertEqual(request(base, 'POST', '/console/api/v1/projects', {'name':'bad'}, {'X-Feynman-Console':token})[0], 403)
            self.assertEqual(request(base, 'POST', '/v1/projects', {'name':'bad'}, {'Origin':base})[0], 401)
            self.assertEqual(cli(base, 'projects', 'list')['projects'], [])
            for path, body in [('access/connections', {'provider':'slate','token':'DO_NOT_REGISTER'}),
                               ('projects/a/workspace/seed', {}),
                               ('projects/a/access/invoke/git.workspace.clone', {}),
                               ('projects/a/resources', {'operation':'secret.put','arguments':{}})]:
                self.assertEqual(browser(base,token,'POST',path,body)[0], 403)

    def test_browser_project_chat_approval_schedule_source_and_restart(self):
        with platform() as (base, root, start, stop):
            token = request(base, 'GET', '/console/session')[2]['csrf_token']
            created = browser(base, token, 'POST', 'projects', {'name':'Console integration'})
            self.assertEqual(created[0], 201, created)
            p = created[2]['project']['id']; agent = created[2]['project']['main_agent_id']
            self.assertEqual(cli(base,'projects','list')['projects'][0]['id'],p)
            source = browser(base, token, 'POST', f'projects/{p}/sources',
                             {'name':'Test source','kind':'manual','agent_id':agent,'enabled':False,'arguments':{}})
            self.assertEqual(source[0], 200, source)
            source_id = source[2]['source']['id']
            schedule = browser(base,token,'POST',f'projects/{p}/schedules',
                               {'title':'Hourly','prompt':'Inspect only','interval_seconds':3600,'enabled':False,'agent_id':agent})
            self.assertEqual(schedule[0],201,schedule)
            self.assertFalse(schedule[2]['schedule']['enabled'])
            submitted = browser(base,token,'POST',f'projects/{p}/inbox',
                                {'agent_id':agent,'content':'BLOCK: ask for inspection approval','delivery':'queue'})
            self.assertEqual(submitted[0],200,submitted)
            status = wait_for(lambda:cli(base,'status',p),lambda s:s['main_agent']['active_run'] and s['main_agent']['active_run']['status']=='waiting_approval')
            run = status['main_agent']['active_run']['id']
            events = browser(base,token,'GET',f'projects/{p}/runs/{run}/events?after=0')[2]['events']
            approval = next(e for e in events if e['kind']=='approval_requested')['payload']['requestId']
            self.assertEqual(browser(base,token,'POST',f'projects/{p}/runs/{run}/approvals/{quote(approval,safe="")}',{'decision':'accept'})[0],204)
            snapshot = wait_for(lambda:browser(base,token,'GET',f'projects/{p}/conversation')[2],lambda c:c['active_run'] is None)
            self.assertIn('fixture reply',snapshot['messages'][-1]['content'])
            self.assertEqual(browser(base,token,'POST',f'projects/{p}/resources',{'operation':'memory.search','arguments':{}})[0],200)
            records_before = browser(base,token,'GET',f'projects/{p}/sources/{source_id}/records?after=0')[2]
            self.assertEqual(records_before['records'],[])
            stop();start()
            self.assertEqual(browser(base,token,'GET',f'projects/{p}/status')[0],403)
            fresh = request(base,'GET','/console/session')[2]['csrf_token']
            self.assertNotEqual(fresh,token)
            after = browser(base,fresh,'GET',f'projects/{p}/conversation')[2]
            self.assertEqual(after['conversation']['runtime_thread_id'],snapshot['conversation']['runtime_thread_id'])
            self.assertEqual(len(after['runs']),1)
            self.assertFalse(browser(base,fresh,'GET',f'projects/{p}/schedules')[2]['schedules'][0]['enabled'])


if __name__ == '__main__': unittest.main()
