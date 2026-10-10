"""Diagnostic-only read bridge, not Feynman's production WorkspaceProvider.

The host owns credentials and the assigned Pod. Callers cannot choose a cluster,
namespace, Pod, shell command, or arbitrary file. No cloud resource is provisioned.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

FILES = {"probe.json", "note.txt"}


def remote(operation, path=None):
    pod = os.environ["FEYNMAN_SMOKE_POD"]
    namespace = os.environ["FEYNMAN_SMOKE_NAMESPACE"]
    if not all(re.fullmatch(r"[a-z0-9][a-z0-9.-]*", x) for x in (pod, namespace)):
        raise ValueError("Invalid host-assigned sandbox")
    # All remote code is static. The only argument is an allowlisted filename.
    code = """
import json, pathlib, socket, sys
root = pathlib.Path('/workspace').resolve()
if sys.argv[1] == 'list':
    print(json.dumps({'directory': '/workspace', 'files': sorted(p.name for p in root.iterdir()), 'hostname': socket.gethostname()}))
else:
    path = root / sys.argv[2]
    if path.is_symlink() or path.resolve().parent != root:
        raise ValueError('Symlink or path outside workspace')
    with path.open('rb') as f:
        data = f.read(8193)
    if len(data) > 8192:
        raise ValueError('File exceeds smoke size limit')
    print(json.dumps({'path': '/workspace/' + path.name, 'content': data.decode(), 'hostname': socket.gethostname()}))
"""
    args = ["kubectl", "--kubeconfig", os.environ["FEYNMAN_SMOKE_KUBECONFIG"],
            "--namespace", namespace, "--request-timeout=30s", "exec", pod,
            "--container", "runtime", "--", "python3", "-c", code, operation]
    if path:
        args.append(path)
    run = subprocess.run(args, capture_output=True, text=True, timeout=45)
    if run.returncode:
        # Do not forward unfiltered native stderr or credential-helper output.
        raise RuntimeError("Remote sandbox command failed; inspect host diagnostics")
    result = json.loads(run.stdout)
    audit = os.environ.get("FEYNMAN_SMOKE_AUDIT")
    if audit:
        with Path(audit).open("a") as f:
            f.write(json.dumps({"operation": operation, "path": path, "pod": pod,
                               "result_sha256": hashlib.sha256(run.stdout.encode()).hexdigest()}) + "\n")
    return result


def handle(method, params, initialized):
    if method == "initialize":
        return {"protocolVersion": params.get("protocolVersion", "2025-03-26"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "feynman-gke-smoke", "version": "0.1.0"}}
    if method == "ping":
        return {}
    if not initialized:
        raise ValueError("Initialize handshake required")
    if method == "tools/list":
        annotation = {"readOnlyHint": True, "destructiveHint": False,
                      "idempotentHint": True, "openWorldHint": True}
        return {"tools": [
            {"name": "sandbox_list_files", "description": "List files in the assigned remote GKE sandbox /workspace.",
             "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}, "annotations": annotation},
            {"name": "sandbox_read_file", "description": "Read probe.json or note.txt from the assigned remote GKE sandbox, never the host filesystem.",
             "inputSchema": {"type": "object", "properties": {"path": {"type": "string", "enum": sorted(FILES)}},
                             "required": ["path"], "additionalProperties": False}, "annotations": annotation},
        ]}
    if method != "tools/call":
        raise ValueError("Unknown method")
    try:
        name, arguments = params.get("name"), params.get("arguments", {})
        if not isinstance(arguments, dict):
            raise ValueError("Arguments must be an object")
        if name == "sandbox_list_files" and not arguments:
            result = remote("list")
        elif name == "sandbox_read_file":
            path = arguments.get("path")
            if set(arguments) != {"path"} or not isinstance(path, str) or path not in FILES:
                raise ValueError("Path outside the assigned workspace allowlist")
            result = remote("read", path)
        else:
            raise ValueError("Unknown tool or unsupported arguments")
        return {"content": [{"type": "text", "text": json.dumps(result)}], "isError": False}
    except (ValueError, KeyError, RuntimeError, OSError, subprocess.TimeoutExpired):
        return {"content": [{"type": "text", "text": "Request rejected or remote access failed: outside the assigned workspace allowlist, invalid arguments, or unavailable sandbox."}], "isError": True}


def main():
    initialized = False
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        try:
            method = request.get("method")
            result = handle(method, request.get("params", {}), initialized)
            initialized = initialized or method == "initialize"
            reply = {"jsonrpc": "2.0", "id": request["id"], "result": result}
        except ValueError as error:
            reply = {"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32602, "message": str(error)}}
        print(json.dumps(reply), flush=True)


if __name__ == "__main__":
    main()
