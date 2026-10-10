"""Fixed bootstrap run by the owner-only Rust endpoint, never an Agent tool."""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

root = Path(sys.argv[1]).resolve()
source = Path(sys.argv[2])
seed = json.loads(source.read_text())
targets = ("repo", "facts", "scratch", "output", "seed-manifest.json")
if any((root / name).exists() or (root / name).is_symlink() for name in targets):
    raise RuntimeError("existing workspace content is never replaced")
stage = Path(tempfile.mkdtemp(prefix=".feynman-seed-", dir=root))
try:
    repo = stage / "repo"
    repo.mkdir()
    for item in seed["files"]:
        relative = item["path"]
        parts = relative.split("/")
        if any(part in ("", ".", "..") or part.lower() == ".git" for part in parts) or "\\" in relative:
            raise RuntimeError("unsafe path")
        content = base64.b64decode(item["content_base64"], validate=True)
        if hashlib.sha256(content).hexdigest() != item["sha256"]:
            raise RuntimeError("file hash mismatch")
        path = repo / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(content)
        path.chmod(0o755 if item["executable"] else 0o644)
    env = {key: os.environ[key] for key in ("PATH", "SYSTEMROOT") if key in os.environ}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_AUTHOR_NAME="Feynman baseline", GIT_AUTHOR_EMAIL="baseline@feynman.local",
               GIT_COMMITTER_NAME="Feynman baseline", GIT_COMMITTER_EMAIL="baseline@feynman.local",
               GIT_AUTHOR_DATE="2000-01-01T00:00:00Z", GIT_COMMITTER_DATE="2000-01-01T00:00:00Z")
    def git(*args):
        return subprocess.check_output(["git", "-c", "core.hooksPath=/dev/null", "-C", str(repo), *args], env=env, stderr=subprocess.DEVNULL, text=True).strip()
    git("init", "-q", "--template=")
    git("add", "-f", ".")
    git("-c", "commit.gpgsign=false", "commit", "-qm", "Isolated analysis baseline")
    manifest = {"base_sha":seed["base_sha"], "initialized_sha":git("rev-parse", "HEAD"),
                "snapshot_sha256":hashlib.sha256(source.read_bytes()).hexdigest(),
                "file_count":len(seed["files"]), "cutoff":seed["facts"]["cutoff"],
                "excluded_prefixes":seed["excluded_prefixes"],
                "write_boundary":"advisory_read_only_with_post_run_integrity_check"}
    (stage / "facts").mkdir()
    (stage / "facts/messages.json").write_text(json.dumps(seed["facts"], ensure_ascii=False))
    (stage / "scratch").mkdir()
    (stage / "output").mkdir()
    (stage / "seed-manifest.json").write_text(json.dumps(manifest))
    # Do not overstate chmod: the trusted Docker Agent currently runs as root.
    # Actual evaluation additionally checks all source hashes and Git status.
    for name in targets:
        if (root / name).exists() or (root / name).is_symlink():
            raise RuntimeError("workspace changed during seeding")
    for name in targets:
        (stage / name).rename(root / name)
    for directory in (root / "repo", root / "facts"):
        for path in directory.rglob("*"):
            path.chmod(0o555 if path.is_dir() or path.stat().st_mode & 0o111 else 0o444)
        directory.chmod(0o555)
    print(json.dumps(manifest), flush=True)
finally:
    # Only the newly owned staging directory, never existing project content.
    for path in stage.rglob("*"):
        if path.is_dir():
            path.chmod(0o700)
    shutil.rmtree(stage)
    source.unlink(missing_ok=True)
