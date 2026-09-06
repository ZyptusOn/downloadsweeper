#!/usr/bin/env python3
"""Offline credential check for source exports and portable directories/ZIPs.

Python 3.11+, standard library only. Prints locations and rule names, never values.
Checks current files, not Git history or whether a provider has revoked a key.
"""

import argparse
import io
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import zipfile


PRIVATE_DIRS = {'.git', '.ds-data', 'artifacts', 'target', 'dist', '__pycache__',
                'node_modules', '.venv', 'venv', '.pytest_cache', '.mypy_cache',
                '.ruff_cache', '%systemdrive%'}
PRIVATE_NAMES = {'config.toml', 'dir_class_overrides.json', 'classification_cache.json',
                 'trajectory.jsonl', '.ds_store', 'thumbs.db', 'desktop.ini', '.ds-workspace.json'}
SAFE_VALUES = {'', 'fixture-search-key', 'YOUR_API_KEY', 'your-api-key'}
KEY_NAME = re.compile(r'(?i)(?:api[_-]?key|token|secret|password)$')
ASSIGNMENT = re.compile(
    r'''(?ix)(?:["']?\b(?:api[_-]?key|[a-z][a-z0-9_]*_api_key|access_token|client_secret|password)["']?)
    \s*[:=]\s*(?:"([^"\r\n]*)"|'([^'\r\n]*)'|([^\s,\#}]+))'''
)
PATTERNS = {
    'provider API key': re.compile(rb'\bsk-[A-Za-z0-9_-]{20,}'),
    'GitHub token': re.compile(rb'\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})'),
    'AWS access key': re.compile(rb'\b(?:AKIA|ASIA)[A-Z0-9]{16}\b'),
    'private key': re.compile(rb'-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----'),
}


def private_path(path):
    parts = [part.lower() for part in path.parts]
    return (any(part in PRIVATE_DIRS for part in parts)
            or any(part.startswith('.sync-') for part in parts)
            or (bool(parts) and parts[-1] in PRIVATE_NAMES)
            or any(a == 'src-tauri' and b == 'gen' for a, b in zip(parts, parts[1:]))
            or (bool(parts) and parts[-1].startswith('session-') and parts[-1].endswith('.json'))
            or any((part == '.env' or part.startswith('.env.'))
                   and part != '.env.example' for part in parts))


def known_credentials(root):
    """Read local secrets only for exact matching; never emit or persist them."""
    values = [value for name, value in os.environ.items() if KEY_NAME.search(name)]
    custom_names = set()
    for name in ('config.toml', 'config.example.toml'):
        path = root / name
        if path.exists():
            config = tomllib.loads(path.read_text(encoding='utf-8-sig'))
            for section in ('llm', 'search'):
                entry = config.get(section, {})
                values.append(entry.get('api_key', ''))
                env_name = entry.get('api_key_env')
                if env_name:
                    custom_names.add(env_name)
                    values.append(os.environ.get(env_name, ''))
    for path in root.glob('.env*'):
        if path.is_file() and path.name != '.env.example':
            for line in path.read_text(encoding='utf-8-sig').splitlines():
                if '=' in line and not line.lstrip().startswith('#'):
                    name, value = line.split('=', 1)
                    if KEY_NAME.search(name.strip()) or name.strip().startswith('DS_') or name.strip() in custom_names:
                        values.append(value.strip().strip('\"\''))
    return {value.encode() for value in values
            if isinstance(value, str) and value.strip() not in SAFE_VALUES and len(value) >= 8}


class Checker:
    def __init__(self, secrets=()):
        self.secrets = set(secrets)
        self.failures = []
        self.files = 0

    def fail(self, location, rule):
        # Also redact a credential if someone used it in a filename.
        for secret in self.secrets:
            location = location.replace(secret.decode(errors='replace'), '[REDACTED]')
        for pattern in PATTERNS.values():
            location = pattern.sub(b'[REDACTED]', location.encode()).decode(errors='replace')
        self.failures.append(f'{location}: {rule}')

    def data(self, location, data, depth=0):
        self.files += 1
        if any(secret in data or secret.decode(errors='replace').encode('utf-16-le') in data
               for secret in self.secrets):
            self.fail(location, 'known local/environment credential')
        # Scan binaries too; UTF-16 is common in Windows artifacts.
        normalized = data.replace(b'\x00', b'')
        for rule, pattern in PATTERNS.items():
            if pattern.search(normalized):
                self.fail(location, rule)
        if '\x00' not in data.decode('utf-8', errors='replace'):
            for number, line in enumerate(data.decode('utf-8-sig', errors='replace').splitlines(), 1):
                if line.lstrip().startswith(('#', '//')):
                    continue
                for match in ASSIGNMENT.finditer(line):
                    value = next(group for group in match.groups() if group is not None)
                    # Unquoted source expressions are not literal credentials.
                    if match.group(3) is not None and not re.fullmatch(r'[A-Za-z0-9_./+\-=]+', value):
                        continue
                    config_literal = location.lower().endswith(('.toml', '.yaml', '.yml', '.env', '.env.example'))
                    env_assignment = re.match(r'(?:export\s+)?[A-Z][A-Z0-9_]*_API_KEY\s*=', line.lstrip())
                    if value not in SAFE_VALUES and (match.group(3) is None or config_literal or env_assignment):
                        self.fail(f'{location}:{number}', 'nonempty credential assignment')
        if location.lower().endswith('.zip'):
            if depth >= 4:
                self.fail(location, 'archive nesting exceeds verification limit')
                return
            try:
                with zipfile.ZipFile(io.BytesIO(data)) as archive:
                    for entry in archive.infolist():
                        if entry.is_dir():
                            continue
                        child = Path(entry.filename.replace('\\', '/'))
                        label = f'{location}!{entry.filename}'
                        if private_path(child) or child.name.lower() == 'config.toml':
                            self.fail(label, 'private/runtime file in portable archive')
                        self.data(label, archive.read(entry), depth + 1)
            except (OSError, ValueError, RuntimeError, zipfile.BadZipFile):
                self.fail(location, 'cannot inspect ZIP')

    def file(self, path, location):
        if path.is_symlink() or (hasattr(path, 'is_junction') and path.is_junction()):
            self.fail(location, 'link cannot be verified as a shareable file')
            return
        try:
            self.data(location, path.read_bytes())
        except OSError:
            self.fail(location, 'cannot read file')

    def package(self, path):
        if path.is_symlink() or (hasattr(path, 'is_junction') and path.is_junction()):
            self.fail(str(path), 'link cannot be verified as a portable package')
        elif path.is_file():
            if path.suffix.lower() != '.zip':
                self.fail(str(path), 'expected portable directory or ZIP')
            else:
                self.file(path, str(path))
        elif path.is_dir():
            for folder, dirs, files in os.walk(path, followlinks=False, onerror=lambda _: self.fail(str(path), 'cannot read directory')):
                for name in dirs[:]:
                    child = Path(folder) / name
                    if child.is_symlink() or (hasattr(child, 'is_junction') and child.is_junction()):
                        self.fail(str(child), 'link in portable directory')
                        dirs.remove(name)
                for name in files:
                    child = Path(folder) / name
                    if private_path(child.relative_to(path)) or name.lower() == 'config.toml':
                        self.fail(str(child), 'private/runtime file in portable directory')
                    self.file(child, str(child))
        else:
            self.fail(str(path), 'package does not exist')


def project_files(root):
    # Include tracked files even if .gitignore now excludes them.
    tracked = set()
    if (root / '.git').exists():
        result = subprocess.run(['git', '-C', str(root), 'ls-files', '-z'], capture_output=True, check=True)
        tracked = {Path(os.fsdecode(name)) for name in result.stdout.split(b'\0') if name}
    files = set(tracked)
    # The private working config is not shareable, but still inspect it for legacy
    # inline credentials. Environment files supply exact-match secrets separately.
    if (root / 'config.toml').is_file():
        files.add(Path('config.toml'))
    def walk_error(error):
        raise error

    for folder, dirs, names in os.walk(root, followlinks=False, onerror=walk_error):
        relative = Path(folder).relative_to(root)
        dirs[:] = [name for name in dirs if not private_path(relative / name)]
        for name in dirs[:]:
            child = Path(folder) / name
            if child.is_symlink() or (hasattr(child, 'is_junction') and child.is_junction()):
                files.add(relative / name)
                dirs.remove(name)
        for name in names:
            path = relative / name
            if not private_path(path) and not name.endswith(('.pyc', '.log')) and name != '.DS_Store':
                files.add(path)
    return sorted(files), tracked


def check_index(root, checker):
    """Inspect staged blobs too: cleaning a working file does not clean its Git index."""
    if not (root / '.git').exists():
        return
    result = subprocess.run(['git', '-C', str(root), 'ls-files', '--stage', '-z'],
                            capture_output=True, check=True)
    entries = []
    for record in result.stdout.split(b'\0'):
        if not record:
            continue
        metadata, name = record.split(b'\t', 1)
        mode, object_id, stage = metadata.split()
        path = Path(os.fsdecode(name))
        if mode not in (b'100644', b'100755') or stage != b'0':
            checker.fail(str(path), 'unsupported or conflicted staged entry')
            continue
        if private_path(path):
            checker.fail(str(path), 'private/runtime file staged in Git')
        entries.append((object_id, path))
    if not entries:
        return
    result = subprocess.run(['git', '-C', str(root), 'cat-file', '--batch'],
                            input=b''.join(object_id + b'\n' for object_id, _ in entries),
                            capture_output=True, check=True)
    stream = io.BytesIO(result.stdout)
    for expected_id, path in entries:
        object_id, kind, size = stream.readline().split()
        if object_id != expected_id or kind != b'blob':
            raise ValueError('unexpected Git object')
        data = stream.read(int(size))
        if len(data) != int(size) or stream.read(1) != b'\n':
            raise ValueError('incomplete Git object')
        checker.data('Git index/' + str(path), data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[1])
    scope = parser.add_mutually_exclusive_group()
    scope.add_argument('--package', action='append', type=Path, help='Check this directory/ZIP instead of all dist packages; repeatable')
    scope.add_argument('--project-only', action='store_true', help='Only check project files (used before building)')
    args = parser.parse_args()
    root = args.root.resolve()
    try:
        if not root.is_dir():
            raise OSError('missing root')
        checker = Checker(known_credentials(root))
        files, tracked = project_files(root)
        for relative in files:
            if relative in tracked and private_path(relative):
                checker.fail(str(relative), 'private/runtime file tracked by Git')
            checker.file(root / relative, str(relative))
        check_index(root, checker)
        if args.project_only:
            packages = []
        elif args.package:
            packages = args.package
        else:
            packages = sorted(path for path in (root / 'dist').glob('*')
                              if path.is_dir() or path.suffix.lower() == '.zip')
        for package in packages:
            checker.package(package)
    except (OSError, ValueError, subprocess.SubprocessError):
        print('FAIL: cannot enumerate project or read credential sources (details suppressed).')
        return 1
    for failure in checker.failures:
        print(f'FAIL: {failure}')
    if checker.failures:
        print(f'FAILED: {len(checker.failures)} findings; credential values suppressed.')
        return 1
    print(f'PASS: {len(files)} project files, {len(packages)} portable directories/ZIPs; {checker.files} files inspected.')
    print('Scope: current shareable files + config.toml; private .env, history, caches and task data are not shareable.')
    if not packages and not args.project_only:
        print('No portable packages found; build one or supply --package to verify a release.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
