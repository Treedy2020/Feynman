"""Trusted operator fixture for native Codex Executor; never exposed as a tool."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile

EXECUTOR = "/opt/codex/vendor/aarch64-unknown-linux-musl/bin/codex"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["prepare", "inspect", "restart", "stop"])
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--package-dir", type=Path, required=True)
    args = parser.parse_args()
    root = args.run_dir.resolve()
    if root.parent != Path("/tmp") or not root.name.startswith("feynman-os-run-"):
        raise RuntimeError("not a fixture directory")
    import docker
    client = docker.from_env()
    manifest = json.loads((root / "manifest.json").read_text())
    payload = json.load(sys.stdin)
    containers = client.containers.list(all=True, filters={"label": "feynman-smoke=" + manifest["run_id"]})
    matches = [c for c in containers if c.labels.get("opensandbox.io/id") == payload["sandbox_id"]]
    if len(matches) != 1:
        raise RuntimeError("sandbox not owned by fixture")
    container = matches[0]
    if args.action == "prepare":
        vendor = args.package_dir / "vendor/aarch64-unknown-linux-musl"
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w") as tar:
            for name in ["bin/codex", "bin/codex-code-mode-host", "codex-path/rg", "codex-resources/bwrap", "codex-resources/zsh/bin/zsh"]:
                tar.add(vendor / name, arcname="codex/vendor/aarch64-unknown-linux-musl/" + name)
        assert container.put_archive("/opt", archive.getvalue())
        if "canary" in payload:
            canary = root / "host-canary.txt"
            canary.write_text(payload["canary"])
            canary.chmod(0o600)
        if "marker" in payload:
            files = {"probe.json": json.dumps({"marker": payload["marker"]}), "calculator.py": "def add(a, b):\n    return a - b\n",
                     "test_calculator.py": "import unittest\nfrom calculator import add\nclass Addition(unittest.TestCase):\n    def test_addition(self):\n        self.assertEqual(add(2, 3), 5)\n"}
            seed = "import json,sys;from pathlib import Path;p=Path('/workspace');d=json.loads(sys.argv[1]);[(p/n).write_text(v) for n,v in d.items()]"
            result = container.exec_run(["python3", "-c", seed, json.dumps(files)])
            assert result.exit_code == 0, result.output.decode()
    if args.action in ["prepare", "restart", "stop"]:
        stop = "import os,signal;from pathlib import Path;p=Path('/tmp/native-executor.pid');\nif p.exists():\n pid=int(p.read_text());cmd=Path('/proc/'+str(pid)+'/cmdline');\n if cmd.exists() and b'exec-server' in cmd.read_bytes(): os.kill(pid,signal.SIGTERM)\n p.unlink(missing_ok=True)"
        result = container.exec_run(["python3", "-c", stop])
        assert result.exit_code == 0, result.output.decode()
        if args.action != "stop":
            start = "import subprocess;from pathlib import Path;log=open('/tmp/native-executor.log','ab');p=subprocess.Popen(['" + EXECUTOR + "','exec-server','--listen','ws://0.0.0.0:44773','--concurrent-requests','8'],cwd='/workspace',stdin=subprocess.DEVNULL,stdout=log,stderr=log,start_new_session=True);Path('/tmp/native-executor.pid').write_text(str(p.pid))"
            result = container.exec_run(["python3", "-c", start])
            assert result.exit_code == 0, result.output.decode()
    container.reload()
    bindings = container.attrs["HostConfig"]["PortBindings"]
    assert all(b["HostIp"] == "127.0.0.1" for values in bindings.values() for b in values)
    assert not container.attrs["HostConfig"]["Privileged"]
    assert not any(m["Type"] == "bind" for m in container.attrs["Mounts"])
    assert any(m.get("Name") == manifest["volume"] and m["Destination"] == "/workspace" for m in container.attrs["Mounts"])
    addresses = [n["IPAddress"] for n in container.attrs["NetworkSettings"]["Networks"].values() if n["IPAddress"]]
    assert len(addresses) == 1
    inspect = "import json,socket;from pathlib import Path;p=Path('/workspace');print(json.dumps({'hostname':socket.gethostname(),'files':{n.name:n.read_text() for n in p.iterdir() if n.is_file() and n.suffix in ['.py','.json','.txt']},'executor_log':Path('/tmp/native-executor.log').read_text()[-12000:]}))"
    result = container.exec_run(["python3", "-c", inspect])
    assert result.exit_code == 0, result.output.decode()
    canary = root / "host-canary.txt"
    print(json.dumps({"container_id": container.id, "address": addresses[0],
                      "host_canary_sha256": hashlib.sha256(canary.read_bytes()).hexdigest() if canary.exists() else None,
                      **json.loads(result.output)}))


if __name__ == "__main__":
    main()
