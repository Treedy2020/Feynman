#!/usr/bin/env python3
"""Operator-only local Docker workspace; no SSH, credential export or volume deletion."""
import argparse
import base64
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tarfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
OPENSANDBOX_VERSION = '0.2.3'
EXECD_IMAGE = 'sandbox-registry.cn-zhangjiakou.cr.aliyuncs.com/opensandbox/execd:v1.0.22'


def private_dir(path):
    if path.is_symlink() or (path.exists() and not path.is_dir()):
        raise RuntimeError('private directory must not be a link or file')
    path.mkdir(parents=True, mode=0o700, exist_ok=True)
    if path.stat().st_mode & 0o077:
        raise RuntimeError('local runtime directories must be private (0700)')


def write_private(path, value, replace=False):
    if path.is_symlink() or (path.exists() and not path.is_file()):
        raise RuntimeError('private file must be regular')
    if path.exists() and not replace:
        raise RuntimeError('refusing to replace existing private file')
    temporary = path.with_name(path.name + '.' + secrets.token_hex(6) + '.tmp')
    try:
        with open(temporary, 'xb', opener=lambda p, f: os.open(p, f, 0o600)) as out:
            out.write(value.encode() if isinstance(value, str) else value)
        if replace:
            os.replace(temporary, path)
        else:
            os.link(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def run(args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def read_json(path):
    return json.loads(path.read_text())


def safe_env(home):
    env = dict(os.environ)
    for key in ('FEYNMAN_DATA_DIR', 'FEYNMAN_DB', 'FEYNMAN_WORKSPACE_ROOT',
                'FEYNMAN_OPENSANDBOX_SMOKE_CONFIG', 'FEYNMAN_CODEX_MODEL',
                'OPENAI_API_KEY', 'CODEX_API_KEY', 'CODEX_ACCESS_TOKEN', 'CODEX_HOME', 'CODEX_SQLITE_HOME'):
        env.pop(key, None)
    env['FEYNMAN_HOME'] = str(home)
    native = home / 'runtime/native-cli.json'
    if native.exists() or native.is_symlink():
        if native.is_symlink() or not native.is_file() or native.stat().st_mode & 0o077:
            raise RuntimeError('native CLI configuration must be a private regular file')
        values = read_json(native)
        if set(values) != {'FEYNMAN_NATIVE_PYTHON', 'FEYNMAN_DRLEE_BIN', 'FEYNMAN_GLAB_BIN'}:
            raise RuntimeError('unsupported native CLI operator settings')
        for key, value in values.items():
            if not Path(value).is_absolute() or not Path(value).is_file():
                raise RuntimeError('native CLI paths must select installed local executables')
            env[key] = value
    return env


def prepare_clis(home, args):
    """Install Slate's actual package in an owned venv; no auth or image mutation."""
    package = args.slate_package.resolve(strict=True)
    if not (package / 'pyproject.toml').is_file():
        raise RuntimeError('provide the Slate CLI package directory')
    runtime = home / 'runtime'
    private_dir(runtime)
    venv = runtime / 'native-cli'
    if not (venv / 'bin/python').exists():
        run(['uv', 'venv', '--python', '3.13', venv])
    run(['uv', 'pip', 'install', '--python', venv / 'bin/python', package])
    values = {'FEYNMAN_NATIVE_PYTHON': str(venv / 'bin/python'),
              'FEYNMAN_DRLEE_BIN': str(Path(shutil.which(args.drlee) or args.drlee).resolve(strict=True)),
              'FEYNMAN_GLAB_BIN': str(Path(shutil.which(args.glab) or args.glab).resolve(strict=True))}
    path = runtime / 'native-cli.json'
    if path.exists() and read_json(path) != values:
        raise RuntimeError('existing native CLI settings differ; refusing to overwrite')
    if not path.exists():
        write_private(path,json.dumps(values))
    print(json.dumps({'native_clis_prepared': True, 'authentication_started': False,
                      'restart_feynman_server_required': True}))


def settings(home):
    return read_json(home / 'runtime/profile.json')


def codex_version(executable):
    result = run([executable, '--version'], capture_output=True, text=True, timeout=10)
    version = result.stdout.strip().removeprefix('codex-cli ')
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise RuntimeError('use an explicitly versioned Codex binary')
    return version


def file_digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def pin_codex(home, source, version):
    """Retain the host runtime independently of Homebrew/npm version cleanup."""
    source = source.resolve(strict=True)
    if not source.is_file() or not os.access(source, os.X_OK) or codex_version(source) != version:
        raise RuntimeError('Codex binary does not match the pinned executor version')
    root = home / 'runtime' / ('codex-host-' + version)
    private_dir(root)
    files = [source]
    companion = source.parent / 'codex-code-mode-host'
    if companion.is_file():
        files.append(companion)
    manifest = {}
    for selected in files:
        name = 'codex' if selected == source else 'codex-code-mode-host'
        destination = root / name
        digest = file_digest(selected)
        if destination.exists() or destination.is_symlink():
            if destination.is_symlink() or not destination.is_file() or \
               destination.stat().st_mode & 0o077 or not os.access(destination, os.X_OK) or file_digest(destination) != digest:
                raise RuntimeError('existing pinned Codex differs; refusing to overwrite')
        else:
            temporary = root / (name + '-' + secrets.token_hex(6) + '.tmp')
            try:
                with open(temporary, 'xb', opener=lambda p, f: os.open(p, f, 0o700)) as output, selected.open('rb') as input_stream:
                    shutil.copyfileobj(input_stream, output)
                if file_digest(temporary) != digest:
                    raise RuntimeError('copied Codex binary failed verification')
                os.link(temporary, destination)
            finally:
                temporary.unlink(missing_ok=True)
        manifest[name] = digest
    if codex_version(root / 'codex') != version:
        raise RuntimeError('retained Codex version verification failed')
    manifest_path = root / 'manifest.json'
    if manifest_path.is_symlink() or (manifest_path.exists() and read_json(manifest_path) != manifest):
        raise RuntimeError('pinned Codex manifest differs; refusing to overwrite')
    if not manifest_path.exists():
        write_private(manifest_path, json.dumps(manifest))
    return root / 'codex', manifest['codex']


def download_codex_package(runtime, version, system, arch):
    suffix = version if system == 'linux' else version + '-' + system + '-' + arch
    package = runtime / ('codex-' + suffix) / 'package'
    if package.exists():
        return package
    dist = json.loads(run(['npm', 'view', '@openai/codex@' + version + '-' + system + '-' + arch,
                          'dist', '--json'], capture_output=True, text=True, timeout=60).stdout)
    url = dist['tarball']
    if url != f'https://registry.npmjs.org/@openai/codex/-/codex-{version}-{system}-{arch}.tgz':
        raise RuntimeError('unexpected Codex package origin')
    with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read(200 * 1024 * 1024 + 1)
    integrity = 'sha512-' + base64.b64encode(hashlib.sha512(data).digest()).decode()
    if len(data) > 200 * 1024 * 1024 or integrity != dist['integrity']:
        raise RuntimeError('Codex package integrity check failed')
    private_dir(package.parent)
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
        if any(not member.name.startswith('package/') or not (member.isfile() or member.isdir())
               for member in archive.getmembers()):
            raise RuntimeError('unexpected Codex package entries')
        archive.extractall(package.parent, filter='data')
    return package


def repair_codex(home, args):
    profile = settings(home)
    version = profile['codex_version']
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise RuntimeError('invalid pinned Codex version')
    source = args.codex
    if source is None:
        system = {'Darwin':'darwin', 'Linux':'linux'}.get(platform.system())
        arch = {'arm64':'arm64', 'aarch64':'arm64', 'x86_64':'x64', 'amd64':'x64'}.get(platform.machine())
        if not system or not arch:
            raise RuntimeError('unsupported host; provide --codex with the exact pinned version')
        package = download_codex_package(home / 'runtime', version, system, arch)
        binaries = list(package.glob('vendor/*/bin/codex'))
        if len(binaries) != 1:
            raise RuntimeError('Codex package has no unique host executable')
        source = binaries[0]
    executable, digest = pin_codex(home, source, version)
    path = home / 'runtime/profile.json'
    updated = dict(profile, codex=str(executable), codex_sha256=digest)
    if updated != profile:
        backup = home / 'runtime/profile.before-codex-pin.json'
        if not backup.exists():
            write_private(backup, json.dumps(profile))
        write_private(path, json.dumps(updated), replace=True)
    print(json.dumps({'codex_repaired':True, 'codex_version':version,
                      'restart_feynman_server_required':True,
                      'model_auth_changed':False, 'executor_image_changed':False}))


def check_codex(home):
    profile = settings(home)
    executable = Path(profile['codex'])
    expected = home / 'runtime' / ('codex-host-' + profile['codex_version']) / 'codex'
    if executable != expected or executable.is_symlink() or not executable.is_file() or \
       not os.access(executable, os.X_OK) or executable.stat().st_mode & 0o077:
        raise RuntimeError('Codex runtime is missing or not independently retained; run repair-codex, then restart')
    if file_digest(executable) != profile.get('codex_sha256') or codex_version(executable) != profile['codex_version']:
        raise RuntimeError('pinned Codex verification failed; inspect runtime or run repair-codex before restarting')
    manifest_path = executable.parent / 'manifest.json'
    if manifest_path.is_symlink() or not manifest_path.is_file():
        raise RuntimeError('pinned Codex manifest missing; run repair-codex before restarting')
    manifest = read_json(manifest_path)
    if not set(manifest).issubset({'codex','codex-code-mode-host'}) or manifest.get('codex') != profile['codex_sha256']:
        raise RuntimeError('invalid pinned Codex manifest; inspect runtime before restarting')
    for name, digest in manifest.items():
        path = executable.parent / name
        if path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK) or \
           path.stat().st_mode & 0o077 or file_digest(path) != digest:
            raise RuntimeError('pinned Codex component verification failed; run repair-codex before restarting')


def prepare(home, args):
    runtime = home / 'runtime'
    private_dir(runtime)
    codex = Path(shutil.which(args.codex) or args.codex).resolve(strict=True)
    version = codex_version(codex)
    codex, digest = pin_codex(home, codex, version)
    profile = {'codex': str(codex), 'codex_sha256':digest, 'codex_version': version,
               'image': 'feynman-codex-executor:' + version + '-local', 'base_image': args.base_image,
               'server_port': args.server_port, 'sandbox_port': args.sandbox_port}
    profile_path = runtime / 'profile.json'
    if profile_path.exists() and read_json(profile_path) != profile:
        raise RuntimeError('existing local runtime has different settings; use its existing pinned configuration')
    run(['docker', 'info'], stdout=subprocess.DEVNULL)
    venv = runtime / 'venv'
    if not (venv / 'bin/python').exists():
        run(['uv', 'venv', '--python', '3.13', venv])
    run(['uv', 'pip', 'install', '--python', venv / 'bin/python', 'opensandbox-server==' + OPENSANDBOX_VERSION])
    arch = run(['docker', 'info', '--format', '{{.Architecture}}'], capture_output=True, text=True).stdout.strip()
    arch = {'aarch64': 'arm64', 'arm64': 'arm64', 'x86_64': 'x64', 'amd64': 'x64'}[arch]
    package = download_codex_package(runtime, version, 'linux', arch)
    if subprocess.run(['docker', 'image', 'inspect', profile['image']], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
        run([venv / 'bin/python', ROOT / 'scripts/opensandbox/build_executor_image.py',
             '--package-dir', package, '--dockerfile', ROOT / 'infra/opensandbox/Dockerfile',
             '--codex-version', version, '--tag', profile['image'], '--base-image', args.base_image])
    run(['docker', 'pull', EXECD_IMAGE])
    if not profile_path.exists():
        write_private(profile_path, json.dumps(profile))
    if not (runtime / 'server.toml').exists():
        key = secrets.token_urlsafe(32)
        write_private(runtime / 'server.toml', f'''[server]
host = "127.0.0.1"
port = {args.sandbox_port}
api_key = "{key}"
max_sandbox_timeout_seconds = 86400
[runtime]
type = "docker"
execd_image = "{EXECD_IMAGE}"
[docker]
network_mode = "bridge"
host_ip = "127.0.0.1"
port_range_min = 48400
port_range_max = 48600
drop_capabilities = ["ALL"]
no_new_privileges = true
pids_limit = 256
[proxy]
resolve_internal = false
[storage]
allowed_host_paths = ["{runtime}/no-host-mounts"]
[log]
level = "WARNING"
''')
        write_private(runtime / 'opensandbox.json', json.dumps({
            'base_url': f'http://127.0.0.1:{args.sandbox_port}/', 'api_key': key,
            'image': profile['image'], 'executor_ssh_target': None,
            'allow_outer_sandbox_only': True, 'timeout_seconds': 1800}))
    if args.import_codex_home:
        source = args.import_codex_home.resolve(strict=True)
        config = source / 'config.toml'
        if config.is_symlink() or not config.is_file() or config.stat().st_size > 1024 * 1024:
            raise RuntimeError('source model config must be a bounded regular file')
        private_dir(home / 'codex')
        destination = home / 'codex/config.toml'
        if not destination.exists():
            write_private(destination, config.read_bytes())
        elif destination.is_symlink() or destination.read_bytes() != config.read_bytes():
            raise RuntimeError('existing local model configuration differs; refusing to overwrite or imply it was imported')
        run([ROOT / 'target/debug/feynman', 'init', '--import-codex-auth', source], env=safe_env(home))
    else:
        run([ROOT / 'target/debug/feynman', 'init'], env=safe_env(home))
    print(json.dumps({'prepared': True, 'home': str(home), 'image': profile['image'],
                      'note': 'Model auth is local only; DWS/business credentials are not imported.'}))


def serve(home):
    from importlib.metadata import version
    if version('opensandbox-server') != OPENSANDBOX_VERSION:
        raise RuntimeError('unexpected OpenSandbox version')
    # Scoped compatibility for pinned 0.2.3, which predates publish_host configuration.
    # Never change the installed package or Docker daemon configuration.
    from opensandbox_server.services.docker import metadata, port_allocator
    from opensandbox_server.services.docker.docker_service import DockerSandboxService
    port_allocator.DOCKER_PUBLISH_HOST = '127.0.0.1'
    port_allocator.PORT_PROBE_HOST = '127.0.0.1'
    metadata.DEFAULT_STORE_DIR = home / 'runtime/metadata'
    # 0.2.3 proxy.py hardcodes resolve_internal=True and ignores [proxy].
    # macOS cannot route to Docker VM bridge IPs. Keep all resolutions on the
    # same ownership-checked host mapping; scoped only to this private process.
    original_endpoint = DockerSandboxService.get_endpoint

    def local_endpoint(service, sandbox_id, port, resolve_internal=False, expires=None):
        return original_endpoint(service, sandbox_id, port, resolve_internal=False, expires=expires)

    DockerSandboxService.get_endpoint = local_endpoint
    from opensandbox_server.cli import main
    sys.argv = ['opensandbox-server', '--config', str(home / 'runtime/server.toml')]
    main()


def process_matches(pid, expected, home):
    result = subprocess.run(['ps', '-p', str(pid), '-o', 'command='], capture_output=True, text=True)
    if result.returncode != 0 or expected not in result.stdout:
        return False
    cwd = subprocess.run(['lsof', '-a', '-p', str(pid), '-d', 'cwd', '-Fn'], capture_output=True, text=True)
    return cwd.returncode == 0 and ('n' + str(home)) in cwd.stdout.splitlines()


def healthy(url):
    try:
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(url, timeout=2) as response:
            return response.status == 200
    except OSError:
        return False


def wait_health(url, process):
    until = time.monotonic() + 30
    while not healthy(url):
        if process.poll() is not None or time.monotonic() > until:
            raise RuntimeError('local service failed to start; inspect its private runtime log')
        time.sleep(.1)


def services(home):
    profile = settings(home)
    env = safe_env(home)
    env.update(FEYNMAN_BIND=f"127.0.0.1:{profile['server_port']}",
               FEYNMAN_CODEX_BIN=profile['codex'],
               FEYNMAN_OPENSANDBOX_CONFIG=str(home / 'runtime/opensandbox.json'))
    return [
        ('sandbox', [str(home / 'runtime/venv/bin/python'), str(Path(__file__).resolve()), '--home', str(home), 'serve'],
         f"http://127.0.0.1:{profile['sandbox_port']}/health", env),
        ('feynman', [str(ROOT / 'target/debug/feynman-server')],
         f"http://127.0.0.1:{profile['server_port']}/v1/health", env)]


def start(home):
    check_codex(home)
    for name, command, health, env in services(home):
        pid_path = home / ('runtime/' + name + '.pid')
        expected = ' '.join(command)
        if pid_path.exists() and process_matches(int(pid_path.read_text()), expected, home):
            if not healthy(health):
                raise RuntimeError('owned local process is running but unhealthy; inspect logs before restarting')
            continue
        with socket.socket() as probe:
            # Graceful restart can leave TIME_WAIT sockets; a live listener still
            # fails this bind, but stale accepted connections must not block up.
            probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            probe.bind(('127.0.0.1', int(health.split(':')[2].split('/')[0])))
        log_path = home / ('runtime/' + name + '.log')
        if not log_path.exists():
            write_private(log_path, b'')
        with log_path.open('ab') as log:
            process = subprocess.Popen(command, env=env, cwd=home, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=log, start_new_session=True)
        write_private(pid_path, str(process.pid), replace=True)
        wait_health(health, process)
    status(home)


def stop(home):
    for name, command, _, _ in reversed(services(home)):
        pid_path = home / ('runtime/' + name + '.pid')
        if not pid_path.exists():
            continue
        pid = int(pid_path.read_text())
        expected = ' '.join(command)
        if process_matches(pid, expected, home):
            os.kill(pid, signal.SIGTERM)
            until = time.monotonic() + 20
            while process_matches(pid, expected, home):
                if time.monotonic() > until:
                    raise RuntimeError('owned service has not stopped; no force kill or data deletion')
                time.sleep(.1)
        pid_path.unlink(missing_ok=True)
    print(json.dumps({'stopped': True, 'note': 'Control processes stopped. Containers/volumes are retained, not deleted.'}))


def status(home):
    profile = settings(home)
    executable = Path(profile['codex'])
    print(json.dumps({'home': str(home), 'server': f"http://127.0.0.1:{profile['server_port']}",
                      'image': profile['image'], 'services': {name: healthy(url) for name, _, url, _ in services(home)},
                      'codex_available':executable.is_file() and os.access(executable, os.X_OK),
                      'note': 'This is local Docker isolation, not gVisor or a strong security boundary.'}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--home', type=Path, default=Path.home() / '.feynman-local-sandbox')
    commands = parser.add_subparsers(dest='action', required=True)
    prep = commands.add_parser('prepare')
    prep.add_argument('--codex', default='codex')
    prep.add_argument('--base-image', default='python:3.12-slim')
    prep.add_argument('--server-port', type=int, default=4318)
    prep.add_argument('--sandbox-port', type=int, default=18081)
    prep.add_argument('--import-codex-home', type=Path)
    native = commands.add_parser('prepare-clis')
    native.add_argument('--slate-package', type=Path, required=True)
    native.add_argument('--drlee', default='drlee')
    native.add_argument('--glab', default='glab')
    repair = commands.add_parser('repair-codex', help='Retain the exact pinned host version without changing credentials or executor')
    repair.add_argument('--codex', type=Path, help='Installed executable of the exact pinned version; otherwise download its verified npm package')
    for name in ['up', 'stop', 'status', 'serve']:
        commands.add_parser(name)
    cli = commands.add_parser('cli')
    cli.add_argument('arguments', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if '..' in args.home.parts or args.home.is_symlink():
        raise RuntimeError('select a dedicated regular local runtime directory')
    home = args.home.absolute()
    if home in (Path('/'), Path.home(), ROOT):
        raise RuntimeError('refusing a filesystem, user or repository root')
    private_dir(home)
    if args.action == 'serve':
        serve(home)
        return
    if args.action == 'cli':
        profile = settings(home)
        arguments = args.arguments[1:] if args.arguments[:1] == ['--'] else args.arguments
        result = subprocess.run([str(ROOT / 'target/debug/feynman'), '--server',
                                 f"http://127.0.0.1:{profile['server_port']}", *arguments], env=safe_env(home), cwd=ROOT)
        sys.exit(result.returncode)
    with open(home / 'runtime.lock', 'a', opener=lambda p, f: os.open(p, f, 0o600)) as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.action == 'prepare':
            prepare(home, args)
        elif args.action == 'prepare-clis':
            prepare_clis(home,args)
        elif args.action == 'repair-codex':
            repair_codex(home,args)
        elif args.action == 'up':
            start(home)
        elif args.action == 'stop':
            stop(home)
        else:
            status(home)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        # Credential-bearing upstream response bodies are never printed.
        print('Local sandbox error: ' + str(error), file=sys.stderr)
        sys.exit(1)
