"""Offline operator safety checks; real Docker/native smoke is explicitly separate."""
import importlib.util
import argparse
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('local_sandbox', Path(__file__).parents[1] / 'opensandbox/local.py')
local = importlib.util.module_from_spec(spec)
spec.loader.exec_module(local)


class LocalSandboxSetupTest(unittest.TestCase):
    def test_runtime_survives_homebrew_removing_the_original_codex(self):
        from test_project_manager import platform, cli, create, FIXTURE
        with tempfile.TemporaryDirectory(prefix='feynman-codex-pin-') as directory:
            home = Path(directory)
            runtime = home / 'runtime'; local.private_dir(runtime)
            upstream = home / 'cask-codex'
            shutil.copyfile(FIXTURE, upstream); upstream.chmod(0o700)
            companion = home / 'codex-code-mode-host'
            companion.write_bytes(b'#!/bin/sh\nexit 0\n'); companion.chmod(0o700)
            original = {'codex':str(upstream), 'codex_version':'0.161.0',
                        'image':'feynman-codex-executor:0.161.0-local',
                        'server_port':4318, 'sandbox_port':18081}
            local.write_private(runtime / 'profile.json', json.dumps(original))
            local.repair_codex(home, argparse.Namespace(codex=upstream))
            upstream.unlink()  # A package manager upgrade removes its old version.
            companion.unlink()
            env = local.services(home)[1][3]
            local.check_codex(home)
            self.assertTrue((Path(env['FEYNMAN_CODEX_BIN']).parent / 'codex-code-mode-host').is_file())
            with platform(codex_override=env['FEYNMAN_CODEX_BIN']) as (base, root, _, _):
                project = create(base, 'Pinned runtime')['id']
                first = cli(base, 'send', project, '--message', 'Read-only fixture probe', '--wait')
                self.assertEqual(first['run']['status'], 'completed', first)
                self.assertEqual(local.settings(home)['image'], original['image'])

    def test_pinned_runtime_rejects_tampering_or_version_drift(self):
        from test_project_manager import FIXTURE
        with tempfile.TemporaryDirectory(prefix='feynman-codex-check-') as directory:
            home = Path(directory)
            runtime = home / 'runtime'; local.private_dir(runtime)
            source = home / 'source-codex'; shutil.copyfile(FIXTURE, source); source.chmod(0o700)
            executable, digest = local.pin_codex(home, source, '0.161.0')
            local.write_private(runtime / 'profile.json', json.dumps({
                'codex':str(executable), 'codex_version':'0.161.0', 'codex_sha256':digest}))
            local.check_codex(home)
            with self.assertRaisesRegex(RuntimeError, 'does not match'):
                local.pin_codex(home, source, '0.162.0')
            executable.write_bytes(executable.read_bytes() + b'\n# tampered\n')
            with self.assertRaisesRegex(RuntimeError, 'verification failed'):
                local.check_codex(home)
            with self.assertRaisesRegex(RuntimeError, 'refusing to overwrite'):
                local.pin_codex(home, source, '0.161.0')

    def test_start_rejects_a_missing_codex_even_when_http_health_is_green(self):
        with tempfile.TemporaryDirectory(prefix='feynman-codex-stale-') as directory:
            home = Path(directory)
            runtime = home / 'runtime'; local.private_dir(runtime)
            local.write_private(runtime / 'profile.json', json.dumps({
                'codex':str(home / 'removed-cask/codex'), 'codex_version':'0.161.0',
                'image':'feynman-codex-executor:0.161.0-local',
                'server_port':4318, 'sandbox_port':18081}))
            for name in ('sandbox', 'feynman'):
                local.write_private(runtime / (name + '.pid'), str(os.getpid()))
            with patch.object(local, 'healthy', return_value=True), \
                 patch.object(local, 'process_matches', return_value=True), \
                 patch.object(local.subprocess, 'Popen') as spawn:
                with self.assertRaisesRegex(RuntimeError, 'repair-codex'):
                    local.start(home)
                spawn.assert_not_called()

    def test_private_files_are_not_linked_or_overwritten(self):
        with tempfile.TemporaryDirectory(prefix='feynman-local-setup-') as directory:
            root=Path(directory)
            local.private_dir(root)
            path=root/'private.json'
            local.write_private(path,b'original')
            self.assertEqual(path.stat().st_mode & 0o777,0o600)
            with self.assertRaises(RuntimeError):
                local.write_private(path,b'replaced')
            self.assertEqual(path.read_bytes(),b'original')
            link=root/'linked.json';link.symlink_to(path)
            with self.assertRaises(RuntimeError):
                local.write_private(link,b'replaced',replace=True)
            self.assertEqual(path.read_bytes(),b'original')

    def test_existing_public_or_linked_directories_fail_closed(self):
        with tempfile.TemporaryDirectory(prefix='feynman-local-setup-') as directory:
            root=Path(directory);public=root/'public';public.mkdir();public.chmod(0o755)
            with self.assertRaises(RuntimeError):
                local.private_dir(public)
            link=root/'linked';link.symlink_to(root,target_is_directory=True)
            with self.assertRaises(RuntimeError):
                local.private_dir(link)

    def test_local_profile_does_not_inherit_personal_auth_or_another_registry(self):
        keys=['FEYNMAN_DATA_DIR','FEYNMAN_DB','FEYNMAN_WORKSPACE_ROOT','FEYNMAN_CODEX_MODEL',
              'FEYNMAN_OPENSANDBOX_SMOKE_CONFIG','OPENAI_API_KEY','CODEX_API_KEY','CODEX_ACCESS_TOKEN','CODEX_HOME','CODEX_SQLITE_HOME']
        with patch.dict(os.environ,{key:'DO_NOT_INHERIT' for key in keys}):
            env=local.safe_env(Path('/private/local-test'))
            self.assertEqual(env['FEYNMAN_HOME'],'/private/local-test')
            self.assertTrue(all(key not in env for key in keys))

    def test_process_with_matching_binary_but_wrong_home_is_not_owned(self):
        with tempfile.TemporaryDirectory(prefix='feynman-local-setup-') as directory:
            self.assertFalse(local.process_matches(os.getpid(),'python',Path(directory)))


if __name__=='__main__':
    unittest.main()
