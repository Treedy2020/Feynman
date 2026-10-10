#!/usr/bin/env python3
"""Offline CLI exercises the real bridge's account isolation/refresh/scrubbing."""
import json
import os
from pathlib import Path
import sys

assert 'OPENAI_API_KEY' not in os.environ
assert 'GITLAB_ACCESS_TOKEN' not in os.environ
home = Path(os.environ['DRLEE_HOME'])
assert os.environ['HOME'] == str(home)
bundle = json.loads((home / 'accounts.json').read_text())
assert len(bundle['accounts']) == 1
assert bundle['active_handle'] == 'test-handle'
account = bundle['accounts'][0]
token = account['credential']['access_token']
assert all(token not in arg for arg in sys.argv)
args = sys.argv[1:]
if args == ['--json','auth']:
    account['credential']['access_token'] = 'ROTATED_NATIVE_SECRET'
    (home / 'accounts.json').write_text(json.dumps(bundle))
    value = {'verified': True, 'email': account['email'], 'endpoint': account['endpoint']['root']}
elif args == ['--json','channel','list']:
    value = [{'id':'11111111-1111-4111-8111-111111111111','description':token}]
    if (home / 'change-identity-on-read').exists():
        account['backend_user_id'] = 'other-user'
        (home / 'accounts.json').write_text(json.dumps(bundle))
elif args[:4] == ['--json','channel','messages','list']:
    assert args[4:] == ['11111111-1111-4111-8111-111111111111','--limit','20']
    value = [{'body':'fixture external observation','sequence':1}]
elif args == ['--json','knowledge','spaces','personal']:
    value = {'id':'space'}
else:
    raise ValueError('unexpected native command')
print(json.dumps({'ok':True, 'data':value, 'refresh_token':account['credential']['refresh_token']}))
