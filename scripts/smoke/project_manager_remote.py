#!/usr/bin/env python3
"""Trusted test operator: inspect/remove only explicit smoke Project resources."""
import argparse
import json
import re
import sys
import base64
import zlib

VERIFY_CODE = r'''
import base64,hashlib,json,os,socket,subprocess,sys,zlib
from pathlib import Path
expected=json.loads(zlib.decompress(base64.b64decode(''.join(sys.argv[1:])))); root=Path('/workspace'); repo=root/'repo'
def git(*args):
    return subprocess.check_output(['git','-c','core.hooksPath=/dev/null','-C',str(repo),*args],text=True).strip()
actual={p.relative_to(repo).as_posix():hashlib.sha256(p.read_bytes()).hexdigest()
        for p in repo.rglob('*') if p.is_file() and '.git' not in p.relative_to(repo).parts}
assert actual=={f['path']:f['sha256'] for f in expected['files']}, 'source hashes/file set changed'
assert json.loads((root/'facts/messages.json').read_text())==expected['facts'], 'facts changed'
assert json.loads((root/'seed-manifest.json').read_text())==expected['manifest'], 'seed manifest changed'
assert git('status','--porcelain','--untracked-files=all')=='', 'source Git dirty'
assert git('rev-list','--count','--all')=='1', 'unexpected history'
assert git('rev-parse','HEAD')==expected['manifest']['initialized_sha'], 'baseline HEAD changed'
assert git('remote')=='', 'remote available'
assert not any(p.is_symlink() for p in repo.rglob('*')), 'linked source'
assert not any((Path('/root')/p).exists() for p in ['.codex/auth.json','.dws','auth.json']), 'host credentials present'
assert not any(os.environ.get(k) for k in ['OPENAI_API_KEY','CODEX_API_KEY','CODEX_ACCESS_TOKEN']), 'model credentials in executor environment'
print(json.dumps({'hostname':socket.gethostname(),'file_count':len(actual),'source_integrity':True,
 'facts_integrity':True,'git_commits':1,'git_remote_absent':True,'host_credentials_absent':True}))
'''


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["inspect", "verify-seed", "remove-container", "cleanup"])
    args = parser.parse_args()
    payload = json.load(sys.stdin)
    import docker
    client = docker.from_env()
    results = []
    for project in payload["projects"]:
        if not re.fullmatch(r"prj_[a-f0-9]{32}", project):
            raise RuntimeError("invalid explicit smoke Project ID")
        environment = "feynman-" + project
        containers = client.containers.list(all=True, filters={"label": "feynman-project=" + project})
        for container in containers:
            if container.labels.get("feynman-environment") != environment or container.labels.get("feynman-tenant") != "local":
                raise RuntimeError("resource ownership mismatch")
            container.reload()
            volume = "feynman-" + project.replace("_", "-")
            assert any(m.get("Name") == volume and m["Destination"] == "/workspace" for m in container.attrs["Mounts"])
            assert not any(m["Type"] == "bind" for m in container.attrs["Mounts"])
            assert not container.attrs["HostConfig"]["Privileged"]
            assert "ALL" in container.attrs["HostConfig"]["CapDrop"]
            assert "no-new-privileges:true" in container.attrs["HostConfig"]["SecurityOpt"]
            assert all(b["HostIp"] == "127.0.0.1" for values in container.attrs["HostConfig"]["PortBindings"].values() for b in values)
            if args.action == "verify-seed":
                encoded=base64.b64encode(zlib.compress(json.dumps(payload['expected']).encode())).decode()
                if len(encoded)>1024*1024:
                    raise RuntimeError('verification manifest exceeds bounded operator input')
                chunks=[encoded[i:i+20000] for i in range(0,len(encoded),20000)]
                result = container.exec_run(["python3", "-c", VERIFY_CODE, *chunks])
                assert result.exit_code == 0, result.output.decode()
                results.append({"project_id": project, "container_id": container.id, **json.loads(result.output)})
            elif args.action == "inspect":
                code = "import json,socket;from pathlib import Path;p=Path('/workspace/manager-probe.json');print(json.dumps({'hostname':socket.gethostname(),'probe':json.loads(p.read_text()) if p.exists() else None,'model_auth_present':Path('/root/.codex/auth.json').exists()}))"
                result = container.exec_run(["python3", "-c", code])
                assert result.exit_code == 0, result.output.decode()
                results.append({"project_id": project, "container_id": container.id, **json.loads(result.output)})
            else:
                container.remove(force=True)
        if args.action == "cleanup":
            try:
                vol = client.volumes.get("feynman-" + project.replace("_", "-"))
                assert vol.attrs["Labels"].get("opensandbox.io/volume-managed-by") == "server"
                vol.remove()
            except docker.errors.NotFound:
                pass
    print(json.dumps({"resources": results, "remaining_containers": sum(len(client.containers.list(all=True, filters={"label": "feynman-project=" + p})) for p in payload["projects"])}))


if __name__ == "__main__":
    main()
