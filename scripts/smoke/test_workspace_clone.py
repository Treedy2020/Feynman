"""Public HTTP and real clone control protocol, with offline Git/Docker upstreams."""
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch
import uuid
from fixtures.opensandbox_lifecycle import upstream
from test_external_access import platform, call, bind, grant, invoke, register, TOKEN


class WorkspaceCloneTest(unittest.TestCase):
    def test_clone_requires_grant_binding_and_never_falls_back_to_host(self):
        with platform() as (base, a, b, root, env):
            args = {'idempotency_key':str(uuid.uuid4())}
            self.assertEqual(invoke(base,a,'git.workspace.clone',args)[0],403)
            grant(base,a,'git.workspace.clone')
            self.assertEqual(invoke(base,a,'git.workspace.clone',args)[0],409)
            connection = register(base)
            self.assertEqual(bind(base,a,connection,'git')[0],200)
            self.assertEqual(invoke(base,a,'git.workspace.clone',{'idempotency_key':'bad'})[0],400)
            self.assertEqual(invoke(base,a,'git.workspace.clone',dict(args,repository='team/other'))[0],400)
            self.assertEqual(invoke(base,a,'git.workspace.clone',args)[0],400)
            workspace = call(base,'POST',f'/v1/projects/{a}/workspace',{'provider':'local'})
            self.assertEqual(workspace[0],200,workspace)
            self.assertEqual(invoke(base,a,'git.workspace.clone',args)[0],400)
            self.assertFalse((Path(workspace[1]['workspace']['cwd'])/'repo').exists())
            self.assertEqual(invoke(base,b,'git.workspace.clone',args)[0],403)

    def test_owned_sandbox_clone_is_idempotent_pinned_and_never_overwrites(self):
        with tempfile.TemporaryDirectory(prefix='feynman-clone-protocol-') as directory:
            transport = Path(directory)
            (transport/'workspace').mkdir();(transport/'bin').mkdir()
            fixtures = Path(__file__).parent/'fixtures'
            docker = transport/'bin/docker'
            docker.write_text((fixtures/'clone_docker.py').read_text().replace(
                "Path(os.environ['FEYNMAN_TEST_CLONE_TRANSPORT'])", 'Path('+repr(directory)+')'))
            docker.chmod(0o700)
            git = transport/'bin/git';shutil.copyfile(fixtures/'clone_git.py',git);git.chmod(0o700)
            with patch.dict(os.environ,{'PATH':str(transport/'bin')+os.pathsep+os.environ['PATH']}):
                with upstream() as (url, state), platform(opensandbox=url) as (base,a,b,root,env):
                    connection = register(base);bind(base,a,connection,'git');grant(base,a,'git.workspace.clone')
                    created = call(base,'POST',f'/v1/projects/{a}/workspace',{'provider':'opensandbox'})
                    self.assertEqual(created[0],200,created)
                    w = created[1]['workspace']
                    (transport/'owned.json').write_text(json.dumps({'volume':w['volume_name'],'labels':{
                        'feynman-project':a,'feynman-tenant':'local','feynman-environment':w['environment_id']}}))
                    args = {'idempotency_key':str(uuid.uuid4())}
                    result = invoke(base,a,'git.workspace.clone',args)
                    self.assertEqual(result[0],200,result)
                    self.assertEqual(result[1]['data']['state'],'cloned')
                    self.assertEqual(result[1]['data']['head'],'a'*40)
                    self.assertFalse(result[1]['data']['credential_persisted'])
                    repo = transport/'workspace/repo'
                    (repo/'source.py').write_text('# Agent development\n')
                    result = invoke(base,a,'git.workspace.clone',args)
                    self.assertEqual(result[0],200,result)
                    self.assertEqual(result[1]['data']['state'],'already_completed')
                    self.assertEqual(invoke(base,a,'git.workspace.clone',{'idempotency_key':str(uuid.uuid4())})[0],500)
                    self.assertEqual((repo/'source.py').read_text(),'# Agent development\n')
                    owned = json.loads((transport/'owned.json').read_text());owned['volume'] = 'wrong-volume'
                    (transport/'owned.json').write_text(json.dumps(owned))
                    self.assertEqual(invoke(base,a,'git.workspace.clone',args)[0],500)
                    self.assertNotIn(TOKEN,(repo/'.git/config').read_text())
                    self.assertNotIn(TOKEN,(transport/'workspace/.feynman-clone.json').read_text())
                    self.assertNotIn(TOKEN,(transport/'docker-argv.jsonl').read_text())
                    self.assertFalse(any(root.rglob('source.py')))


if __name__ == '__main__': unittest.main()
