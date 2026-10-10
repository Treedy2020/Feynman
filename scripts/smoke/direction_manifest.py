"""Independent expected Git identity for the fixed, SHA-1 bootstrap format.

Operator-only: derives metadata from the frozen host snapshot, never trusts an
Agent-controlled manifest. Does not execute Git, hooks, filters or model tools.
"""
import base64
import hashlib
import json

def expected_from_snapshot(path):
    raw=path.read_bytes()
    seed=json.loads(raw)
    def digest(kind,data):
        return hashlib.sha1(kind.encode()+b' '+str(len(data)).encode()+b'\0'+data).digest()
    tree={}
    for file in seed['files']:
        parts=file['path'].split('/');node=tree
        for part in parts[:-1]:
            node=node.setdefault(part,{})
        node[parts[-1]]=(b'100755' if file['executable'] else b'100644',digest('blob',base64.b64decode(file['content_base64'])))
    def build(node):
        entries=[]
        for name,value in sorted(node.items(),key=lambda pair:pair[0].encode()+(b'/' if isinstance(pair[1],dict) else b'')):
            mode,sha=(b'40000',build(value)) if isinstance(value,dict) else value
            entries.append(mode+b' '+name.encode()+b'\0'+sha)
        return digest('tree',b''.join(entries))
    commit=b'tree '+build(tree).hex().encode()+b'\nauthor Feynman baseline <baseline@feynman.local> 946684800 +0000\ncommitter Feynman baseline <baseline@feynman.local> 946684800 +0000\n\nIsolated analysis baseline\n'
    manifest={'base_sha':seed['base_sha'],'initialized_sha':digest('commit',commit).hex(),
              'snapshot_sha256':hashlib.sha256(raw).hexdigest(),'file_count':len(seed['files']),
              'cutoff':seed['facts']['cutoff'],'excluded_prefixes':seed['excluded_prefixes'],
              'write_boundary':'advisory_read_only_with_post_run_integrity_check'}
    return {'files':[{'path':file['path'],'sha256':file['sha256']} for file in seed['files']],
            'facts':seed['facts'],'manifest':manifest}
