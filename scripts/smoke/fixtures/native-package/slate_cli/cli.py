import json
import os

def run(argv, *, token_store, stdout, stderr, stdin, **kwargs):
    assert 'OPENAI_API_KEY' not in os.environ
    assert token_store.get('prod') == 'FAKE_ACCESS_TOKEN_MUST_NOT_LEAK'
    assert argv[:4] == ['--profile', 'prod', '--base-url', 'https://slate.example']
    command = argv[9:]
    assert command in (['me'], ['task','list','--view','mine','--limit','20'], ['project','list'])
    data = {'id':'slate-user'} if command == ['me'] else [{'id':'item'}]
    stdout.write(json.dumps({'data':data, 'errors':[], 'access_token':token_store.get('prod'),
                             'description':token_store.get('prod')}))
    return 0
