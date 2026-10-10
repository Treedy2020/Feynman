#!/usr/bin/env python3
"""Offline DWS process fixture: no OAuth server, real accounts, or real secrets.

Copied to an executable named after a scenario by the integration test. Its wire
output mirrors the observed DWS CLI; extra canary fields exercise redaction.
"""
import json
import os
from pathlib import Path
import sys
import time
import signal
from datetime import datetime,timezone,timedelta

mode = Path(sys.argv[0]).name
Path(sys.argv[0] + ".pid").write_text(str(os.getpid()))
config = Path(os.environ["DWS_CONFIG_DIR"])
config.mkdir(parents=True, exist_ok=True)
assert "DWS_CLIENT_SECRET" not in os.environ
assert os.environ["DWS_KEYCHAIN_DIR"] != str(Path.home() / ".local/share")
assert os.environ["DWS_DISABLE_KEYCHAIN"] == "1"

if sys.argv[1:4] == ["auth", "login", "--device"]:
    assert sys.argv[1:] == ["auth", "login", "--device", "--no-browser", "--format", "json"]
    host = "attacker.example" if mode == "untrusted" else "login.dingtalk.com"
    extra = "?accessToken=FIXTURE_SECRET_MUST_NOT_LEAK" if mode == "secret-uri" else ""
    seconds = 1 if mode == "expired" else 120
    print("\x1b[34m╭────────────────────────────────────────╮\x1b[0m", file=sys.stderr)
    print("│ 请在浏览器中打开以下链接，并输入授权码： │", file=sys.stderr)
    print(f"│   链接: https://{host}/device{extra} │", file=sys.stderr)
    print("│   授权码: ABCD-1234 │", file=sys.stderr)
    print("│ 或者直接打开以下链接： │", file=sys.stderr)
    print(f"│ https://{host}/device?user_code=ABCD-1234 │", file=sys.stderr)
    print(f"│ 授权码将在 {seconds} 秒后过期。 │", file=sys.stderr)
    print("╰────────────────────────────────────────╯", file=sys.stderr, flush=True)
    # This must never be persisted or returned even on failure.
    print("refresh_token=FIXTURE_SECRET_MUST_NOT_LEAK", file=sys.stderr, flush=True)
    if mode in ("waiting", "expired", "untrusted"):
        time.sleep(30)
    else:
        time.sleep(0.4)
    if mode == "denied":
        print("用户拒绝授权 REJECTED", file=sys.stderr)
        sys.exit(3)
    if mode == "malformed":
        print("not-json FIXTURE_SECRET_MUST_NOT_LEAK")
    else:
        print(json.dumps({"success": True, "access_token": "FIXTURE_SECRET_MUST_NOT_LEAK"}))
elif sys.argv[1:3] == ["auth", "status"]:
    count_file = config / "fixture-check-count"
    count = int(count_file.read_text()) + 1 if count_file.exists() else 1
    count_file.write_text(str(count))
    data = {
        "success": True, "authenticated": not (mode == "invalidated" and count > 1),
        "token_valid": True, "corp_id": "fixture-corp", "user_id": "fixture-user",
        "corp_name": "Fixture Org", "user_name": "Fixture User",
        "expires_at": "2099-01-01T00:00:00Z",
        "refresh_token": "FIXTURE_SECRET_MUST_NOT_LEAK",
    }
    if mode == "changed" and count > 1:
        data["corp_id"] = "another-corp"
    print(json.dumps(data))
elif sys.argv[1:3] == ["aisearch", "enterprise"]:
    print(json.dumps({"results": [{"title": "Bound fixture fact"}], "refresh_token": "FIXTURE_SECRET_MUST_NOT_LEAK"}))
elif sys.argv[1:3] == ["chat", "+chat-messages"]:
    assert sys.argv[sys.argv.index('--profile')+1] == 'fixture-corp:fixture-user'
    assert '--page-all' in sys.argv and '--download-resources' not in sys.argv
    start=(datetime.fromisoformat(sys.argv[sys.argv.index('--start')+1])+timedelta(seconds=1)).replace(microsecond=0)
    print(json.dumps({"messages":[{"messageId":"fixture-message","text":"Bound observation","createTime":start.astimezone(timezone(timedelta(hours=8))).strftime('%Y-%m-%d %H:%M:%S')}],"complete":mode!='partial',"hasMore":mode=='partial',"failures":[],"refresh_token":"FIXTURE_SECRET_MUST_NOT_LEAK"}))
elif sys.argv[1:3]==['event','+listen-im']:
    assert sys.argv[sys.argv.index('--profile')+1]=='fixture-corp:fixture-user'
    assert sys.argv[sys.argv.index('--duration')+1]=='10m'
    assert sys.argv[sys.argv.index('--events')+1]=='message'
    def cleanup(*_):
        (config/'listener-cleaned').write_text('SIGTERM')
        sys.exit(0)
    signal.signal(signal.SIGTERM,cleanup)
    target=sys.argv[sys.argv.index('--chat-id')+1] if '--chat-id' in sys.argv else sys.argv[sys.argv.index('--open-dingtalk-id')+1]
    print('[event] ready consumer_id=fixture source=fixture',file=sys.stderr,flush=True)
    for destination in ['cidOutOfScope==',target,target]:
        print(json.dumps({'message_id':'fixture-event','conversation_id':destination,'sender_open_dingtalk_id':destination,'create_time':datetime.now(timezone.utc).isoformat(),'text':'Bound live fact','access_token':'FIXTURE_SECRET_MUST_NOT_LEAK'}),flush=True)
    time.sleep(600)
else:
    sys.exit(2)
