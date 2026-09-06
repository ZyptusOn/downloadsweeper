"""Regression tests use synthetic credentials only; run with Python 3.11+."""

import contextlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import verify_share


class ShareCheckTests(unittest.TestCase):
    def test_repository_outputs_are_private_but_samples_remain_shareable(self):
        for name in ['config.toml', 'dir_class_overrides.json', 'crates/.DS_Store',
                     'src-tauri/gen/schemas/desktop-schema.json', '%SystemDrive%/cache.json',
                     'node_modules/package/index.js', '.venv/bin/python']:
            self.assertTrue(verify_share.private_path(Path(name)), name)
        for name in ['config.example.toml', '.env.example', 'Cargo.lock', 'frontend/vendor/react.production.min.js']:
            self.assertFalse(verify_share.private_path(Path(name)), name)

    def test_index_secret_is_detected_after_working_copy_is_cleaned(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(['git', 'init', '-q', str(root)], check=True, capture_output=True)
            secret = b'staged-only-' + b'private-value-123'
            (root / 'notes.txt').write_bytes(secret)
            subprocess.run(['git', '-C', str(root), 'add', 'notes.txt'], check=True, capture_output=True)
            (root / 'notes.txt').write_text('clean working copy', encoding='utf-8')
            checker = verify_share.Checker([secret])
            verify_share.check_index(root, checker)
            self.assertTrue(any('Git index/' in failure for failure in checker.failures))
            self.assertNotIn(secret.decode(), '\n'.join(checker.failures))

    def test_empty_examples_pass(self):
        checker = verify_share.Checker()
        root = Path(__file__).resolve().parents[1]
        for name in ('config.example.toml', '.env.example'):
            checker.file(root / name, name)
        self.assertEqual(checker.failures, [])

    def test_known_credentials_in_binary_and_utf16(self):
        secret = b'synthetic' + b'-private-credential-123'
        for data in (b'\x00binary' + secret, secret.decode().encode('utf-16-le')):
            checker = verify_share.Checker([secret])
            checker.data('program.exe', data)
            self.assertTrue(checker.failures)
            self.assertNotIn(secret.decode(), '\n'.join(checker.failures))

    def test_unknown_provider_key_and_assignment(self):
        token = b'sk-' + b'a1B2c3D4' * 4
        for data in (b'embedded=' + token, ('api_' + 'key = "opaque-secret"').encode()):
            checker = verify_share.Checker()
            checker.data('config.toml', data)
            self.assertTrue(checker.failures)
            self.assertNotIn('opaque-secret', '\n'.join(checker.failures))
            self.assertNotIn(token.decode(), '\n'.join(checker.failures))

    def test_unquoted_yaml_and_environment_literals(self):
        for name, data in [('settings.yaml', 'api_' + 'key: synthetic-unquoted-value'),
                           ('setup.sh', 'PROVIDER_API_' + 'KEY=synthetic-unquoted-value')]:
            checker = verify_share.Checker()
            checker.data(name, data.encode())
            self.assertTrue(checker.failures)

    def test_compressed_zip_contents_are_checked(self):
        secret = b'only-in-' + b'compressed-content'
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, 'w', zipfile.ZIP_DEFLATED) as archive:
            archive.writestr('portable/program.exe', b'\x00' + secret)
            archive.writestr('portable/.env', '')
        checker = verify_share.Checker([secret])
        checker.data('portable.zip', buffer.getvalue())
        self.assertTrue(any('known local' in failure for failure in checker.failures))
        self.assertTrue(any('private/runtime' in failure for failure in checker.failures))

    def test_missing_or_corrupt_package_fails(self):
        checker = verify_share.Checker()
        checker.data('corrupt.zip', b'not a zip')
        with tempfile.TemporaryDirectory() as temporary:
            checker.package(Path(temporary) / 'missing.zip')
        self.assertEqual(len(checker.failures), 2)

    def test_custom_environment_name_and_private_path_case(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            secret = 'synthetic-' + 'custom-provider-value'
            (root / 'config.toml').write_text('[llm]\napi_key_env = "MY_MODEL_KEY"', encoding='utf-8')
            (root / '.env').write_text('MY_MODEL_KEY=' + secret, encoding='utf-8')
            with patch.dict(os.environ, {}, clear=True):
                self.assertIn(secret.encode(), verify_share.known_credentials(root))
        self.assertTrue(verify_share.private_path(Path('package/.ENV.local')))
        self.assertFalse(verify_share.private_path(Path('package/.env.example')))

    def test_private_env_is_excluded_but_its_key_is_used(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            secret = 'synthetic-' + 'local-value-123'
            (root / '.env').write_text(f"{'DS_API_KEY'}={secret}", encoding='utf-8')
            (root / 'config.toml').write_text('[llm]\napi_key_env = "DS_API_KEY"', encoding='utf-8')
            with patch.dict(os.environ, {}, clear=True):
                secrets = verify_share.known_credentials(root)
            files, _ = verify_share.project_files(root)
            self.assertNotIn(Path('.env'), files)
            self.assertIn(Path('config.toml'), files)
            checker = verify_share.Checker(secrets)
            checker.data('accidental-copy.txt', secret.encode())
            self.assertTrue(checker.failures)

    def test_tracked_ignored_secret_is_not_skipped(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(['git', 'init', '-q', str(root)], check=True, capture_output=True)
            (root / '.gitignore').write_text('.env\n', encoding='utf-8')
            (root / '.env').write_text('', encoding='utf-8')
            subprocess.run(['git', '-C', str(root), 'add', '-f', '.env'], check=True, capture_output=True)
            files, tracked = verify_share.project_files(root)
            self.assertIn(Path('.env'), files)
            self.assertIn(Path('.env'), tracked)
            self.assertTrue(verify_share.private_path(Path('.env')))

    def test_cli_fails_without_echoing_credential(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            secret = 'synthetic-' + 'cli-private-key'
            (root / 'config.toml').write_text('[llm]\n' + 'api_' + 'key = "' + secret + '"', encoding='utf-8')
            output = io.StringIO()
            with patch.object(sys, 'argv', ['verify_share.py', '--root', str(root)]), contextlib.redirect_stdout(output):
                result = verify_share.main()
            self.assertEqual(result, 1)
            self.assertNotIn(secret, output.getvalue())


if __name__ == '__main__':
    unittest.main()
