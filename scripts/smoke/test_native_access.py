"""Public CLI/HTTP/MCP regression for native provider auth and isolation."""
import json
from pathlib import Path
import subprocess
import time
import unittest
from test_external_access import platform,call,grant,invoke,ROOT,TOKEN


class NativeAccessTest(unittest.TestCase):
    def test_slate_native_session_import_binding_and_no_arbitrary_commands(self):
        with platform(native=True) as (base,a,b,root,env):
            cli=[str(ROOT/'target/debug/feynman'),'--server',base,'--json','access']
            result=subprocess.run(cli+['connections','import-slate','--hostname','slate.example'],env=env,capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertNotIn(TOKEN,result.stdout+result.stderr)
            connection=json.loads(result.stdout)['connection']
            self.assertEqual(connection['identity'],'slate-user')
            grant(base,a,'slate.tasks.read')
            self.assertEqual(invoke(base,a,'slate.tasks.read')[0],409)
            self.assertEqual(call(base,'POST',f'/v1/projects/{a}/access/bindings',{'provider':'slate','connection_id':connection['id']})[0],200)
            value=invoke(base,a,'slate.tasks.read')
            self.assertEqual(value[0],200,value)
            self.assertEqual(value[1]['data']['description'],'[redacted]')
            self.assertNotIn('access_token',value[1]['data'])
            self.assertEqual(invoke(base,a,'slate.tasks.read',{'argv':['task','create']})[0],400)
            self.assertEqual(invoke(base,b,'slate.tasks.read')[0],403)
            self.assertEqual(call(base,'PUT',f'/v1/projects/{a}/external-tools/slate.tasks.write')[0],400)
            call(base,'POST',f"/v1/access/connections/{connection['id']}/disable",{})
            self.assertEqual(invoke(base,a,'slate.tasks.read')[0],409)

    def test_drlee_import_copies_one_account_refreshes_privately_and_binds_channel(self):
        with platform(native=True) as (base,a,b,root,env):
            source=root/'original';source.mkdir(mode=0o700)
            account={'handle':'test-handle','endpoint':{'root':'https://drlee.example'},'backend_user_id':'drlee-user','email':'fixture@example.test','credential':{'access_token':TOKEN,'refresh_token':'PRIVATE_REFRESH_SECRET'}}
            bundle={'version':1,'active_handle':'test-handle','accounts':[account,dict(account,handle='other-handle')]}
            path=source/'accounts.json';path.write_text(json.dumps(bundle));path.chmod(0o600)
            original=path.read_bytes()
            cli=[str(ROOT/'target/debug/feynman'),'--server',base,'--json','access']
            result=subprocess.run(cli+['connections','import-drlee','--hostname','drlee.example','--source-home',str(source)],env=env,capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertNotIn(TOKEN,result.stdout+result.stderr)
            connection=json.loads(result.stdout)['connection']
            self.assertEqual(connection['identity'],'drlee-user')
            self.assertEqual(path.read_bytes(),original)
            copied=root/'data/private/external-access'/connection['id']/'accounts.json'
            self.assertEqual(copied.stat().st_mode&0o777,0o600)
            native=json.loads(copied.read_text())
            self.assertEqual(len(native['accounts']),1)
            self.assertEqual(native['accounts'][0]['credential']['access_token'],'ROTATED_NATIVE_SECRET')
            binding={'provider':'drlee','connection_id':connection['id']}
            self.assertEqual(call(base,'POST',f'/v1/projects/{a}/access/bindings',binding)[0],200)
            grant(base,a,'drlee.channels.read');grant(base,a,'drlee.messages.read')
            result=invoke(base,a,'drlee.channels.read')
            self.assertEqual(result[0],200,result)
            self.assertNotIn('PRIVATE_REFRESH_SECRET',json.dumps(result))
            self.assertNotIn('ROTATED_NATIVE_SECRET',json.dumps(result))
            self.assertEqual(invoke(base,a,'drlee.messages.read')[0],400)
            binding['resource_id']='11111111-1111-4111-8111-111111111111'
            call(base,'POST',f'/v1/projects/{a}/access/bindings',binding)
            result=invoke(base,a,'drlee.messages.read')
            self.assertEqual(result[0],200,result)
            self.assertEqual(result[1]['data']['data']['data'][0]['sequence'],1)
            self.assertEqual(invoke(base,a,'drlee.messages.read',{'resource_id':'other'})[0],400)
            self.assertEqual(invoke(base,b,'drlee.messages.read')[0],403)
            # Identity changes during a business read must not release its result.
            marker = copied.parent/'change-identity-on-read';marker.touch()
            self.assertEqual(invoke(base,a,'drlee.channels.read')[0],500)
            marker.unlink()
            # Native cache identity mutation must not silently change Connection identity.
            native['accounts'][0]['backend_user_id']='other-user';copied.write_text(json.dumps(native))
            self.assertEqual(invoke(base,a,'drlee.channels.read')[0],409)
            self.assertEqual(path.read_bytes(),original)

    def test_agent_mcp_discovers_and_requests_but_never_authenticates_or_approves(self):
        with platform(native=True) as (base,a,b,root,env):
            grant(base,a,'external.access')
            env=dict(env,FEYNMAN_DB=str(root/'data/rust-platform.sqlite3'),FEYNMAN_AGENT_PROJECT_ID=a,FEYNMAN_AGENT_USER_ID='local-user',FEYNMAN_AGENT_TENANT_ID='local',FEYNMAN_ACCESS_SERVER_URL=base)
            requests=[{'jsonrpc':'2.0','id':1,'method':'initialize'}, {'jsonrpc':'2.0','id':2,'method':'tools/list'},
                {'jsonrpc':'2.0','id':3,'method':'tools/call','params':{'name':'feynman_access','arguments':{'operation':'request','provider':'drlee','capability':'drlee.channels.read','reason':'Collect project evidence'}}},
                {'jsonrpc':'2.0','id':4,'method':'tools/call','params':{'name':'feynman_access','arguments':{'operation':'register','provider':'slate','token':TOKEN}}}]
            result=subprocess.run([str(ROOT/'target/debug/feynman-server'),'external-tools-mcp'],env=env,input='\n'.join(map(json.dumps,requests))+'\n',capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            replies=list(map(json.loads,result.stdout.splitlines()))
            names=[tool['name'] for tool in replies[1]['result']['tools']]
            self.assertIn('git_workspace_clone',names);self.assertIn('slate_tasks_read',names);self.assertIn('drlee_messages_read',names)
            self.assertFalse(replies[2]['result']['isError'])
            self.assertTrue(replies[3]['result']['isError'])
            self.assertNotIn(TOKEN,result.stdout+result.stderr)
            self.assertEqual(call(base,'GET',f'/v1/projects/{b}/access')[1]['requests'],[])
            self.assertEqual(call(base,'GET','/v1/access/connections')[1]['connections'],[])


if __name__=='__main__': unittest.main()
