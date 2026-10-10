#!/usr/bin/env python3
"""Operator-only Spark fixture helper. Never registered as an Agent tool."""
import argparse
import json
import os
from pathlib import Path
import secrets
import signal
import subprocess
import sys
import time


def private_json(path, value):
    with open(path, "x", opener=lambda p, f: os.open(p, f, 0o600)) as out:
        json.dump(value, out)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["start", "serve", "seed", "inspect", "cleanup"])
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--port", type=int, default=18080)
    args = parser.parse_args()
    root = args.run_dir.resolve()
    if root.parent != Path("/tmp") or not root.name.startswith("feynman-os-run-"):
        raise RuntimeError("refusing non-fixture directory")
    if args.action == "serve":
        # 0.2.3 has no publish_host setting. Scoped, in-memory test override;
        # never modify Docker daemon settings or the installed package files.
        from importlib.metadata import version
        assert version("opensandbox-server") == "0.2.3"
        from opensandbox_server.services.docker import metadata, port_allocator
        port_allocator.DOCKER_PUBLISH_HOST = "127.0.0.1"
        port_allocator.PORT_PROBE_HOST = "127.0.0.1"
        metadata.DEFAULT_STORE_DIR = root / "metadata"
        from opensandbox_server.cli import main as serve
        sys.argv = ["opensandbox-server", "--config", str(root / "server.toml")]
        serve()
        return

    import docker
    client = docker.from_env()
    manifest_path = root / "manifest.json"
    if args.action == "start":
        if manifest_path.exists():
            raise RuntimeError("fixture already started")
        run_id = secrets.token_hex(8)
        volume = "feynman-smoke-" + run_id
        api_key = secrets.token_urlsafe(32)
        private_json(manifest_path, {"run_id": run_id, "volume": volume})
        client.volumes.create(volume, labels={"feynman-smoke": run_id})
        config = f'''[server]
host = "127.0.0.1"
port = {args.port}
api_key = "{api_key}"
max_sandbox_timeout_seconds = 1800
[runtime]
type = "docker"
execd_image = "sandbox-registry.cn-zhangjiakou.cr.aliyuncs.com/opensandbox/execd:v1.0.22"
[docker]
network_mode = "bridge"
port_range_min = 48000
port_range_max = 48200
drop_capabilities = ["ALL"]
no_new_privileges = true
pids_limit = 256
[proxy]
resolve_internal = true
[store]
type = "sqlite"
path = "{root}/opensandbox.db"
[storage]
allowed_host_paths = ["{root}/no-host-mounts"]
[log]
level = "WARNING"
'''
        with open(root / "server.toml", "x", opener=lambda p, f: os.open(p, f, 0o600)) as out:
            out.write(config)
        with open(root / "server.log", "ab") as log:
            proc = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "serve", "--run-dir", str(root)],
                                    stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True)
        manifest = json.loads(manifest_path.read_text())
        manifest["pid"] = proc.pid
        manifest_path.write_text(json.dumps(manifest))
        # Captured by the local runner, never printed to its console or checked in.
        print(json.dumps({**manifest, "api_key": api_key}))
        return

    manifest = json.loads(manifest_path.read_text())
    containers = client.containers.list(all=True, filters={"label": "feynman-smoke=" + manifest["run_id"]})
    if args.action == "cleanup":
        for container in containers:
            assert container.labels.get("opensandbox.io/id")
            container.remove(force=True)
        volume = client.volumes.get(manifest["volume"])
        assert volume.attrs["Labels"]["feynman-smoke"] == manifest["run_id"]
        volume.remove()
        pid = manifest.get("pid")
        if pid and Path(f"/proc/{pid}/cmdline").exists():
            cmdline = Path(f"/proc/{pid}/cmdline").read_bytes()
            assert str(root).encode() in cmdline and b"serve" in cmdline
            os.kill(pid, signal.SIGTERM)
            for _ in range(50):
                if not Path(f"/proc/{pid}").exists():
                    break
                time.sleep(0.1)
        (root / "server.toml").unlink(missing_ok=True)
        print(json.dumps({"cleanup": True,
                          "remaining_containers": len(client.containers.list(all=True, filters={"label": "feynman-smoke=" + manifest["run_id"]})),
                          "remaining_volumes": len(client.volumes.list(filters={"label": "feynman-smoke=" + manifest["run_id"]}))}))
        return

    payload = json.load(sys.stdin)
    matches = [c for c in containers if c.labels.get("opensandbox.io/id") == payload["sandbox_id"]]
    if len(matches) != 1:
        raise RuntimeError("sandbox is not owned by this fixture")
    container = matches[0]
    if args.action == "seed":
        script = "import json,sys; from pathlib import Path; d=json.loads(sys.argv[1]); p=Path('/workspace/smoke'); p.mkdir(exist_ok=True); [(p/n).write_text(v) for n,v in d.items()]"
        files = payload["files"]
        assert set(files) == {"probe.json", "note.txt"}
        result = container.exec_run(["python3", "-c", script, json.dumps(files)])
        assert result.exit_code == 0, "fixture seed failed"
    bindings = container.attrs["HostConfig"]["PortBindings"]
    assert bindings and all(b["HostIp"] == "127.0.0.1" for values in bindings.values() for b in values)
    mounts = container.attrs["Mounts"]
    assert any(m.get("Name") == manifest["volume"] and m["Destination"] == "/workspace" for m in mounts)
    result = container.exec_run(["python3", "-c", "import json; from pathlib import Path; p=Path('/workspace/smoke'); print(json.dumps({n:(p/n).read_text() for n in ['probe.json','note.txt']}))"])
    assert result.exit_code == 0, "independent file verification failed"
    print(json.dumps({"container_id": container.id, "loopback_only": True, "files": json.loads(result.output)}))


if __name__ == "__main__":
    main()
