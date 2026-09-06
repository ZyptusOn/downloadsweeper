#!/usr/bin/env python3
"""One-way source -> build copy, preserving private data and detecting local edits.

Run from the source repository: python -B scripts/sync_workspace.py
Only previously managed, unchanged files may be replaced or removed. No Git data,
credentials, runtime state or build caches are copied. Standard library only.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import tempfile

from verify_share import Checker, known_credentials, private_path, project_files

MARKER = '.ds-workspace.json'


def linked(path):
    if not path.exists() and not path.is_symlink():
        return False
    info = path.lstat()
    return stat.S_ISLNK(info.st_mode) or bool(getattr(info, 'st_file_attributes', 0) & 0x400)


def safe_path(root, name):
    path = Path(name)
    if path.is_absolute() or not path.parts or any(p in ('.', '..') or ':' in p for p in path.parts):
        raise ValueError('Invalid managed path')
    target = root / path
    for part in [target, *target.parents]:
        if linked(part):
            raise ValueError('Refusing a symlink or reparse point: ' + str(part))
        if part == root:
            break
    if not target.resolve().is_relative_to(root):
        raise ValueError('Managed path escapes build directory')
    return target


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None


def atomic_write(path, data, mode=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix='.sync-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        if mode is not None:
            os.chmod(temporary, mode)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def sync(source, destination, check=False):
    source = Path(source).absolute()
    destination = Path(destination).absolute()
    for path in (source, destination):
        if any(linked(p) for p in [path, *path.parents]):
            raise ValueError('Workspace roots must not use links or reparse points')
    source, destination = source.resolve(), destination.resolve()
    if source.is_relative_to(destination) or destination.is_relative_to(source):
        raise ValueError('Source and build directories must be separate, non-nested directories')
    if (source / MARKER).exists():
        raise ValueError('Run synchronization from the source repository, not the build copy')
    if not (source / 'Cargo.toml').is_file():
        raise ValueError('Source is not a Rust project')
    marker = safe_path(destination, MARKER)
    manifest = {'source': str(source), 'files': {}}
    if marker.exists():
        manifest = json.loads(marker.read_text(encoding='utf-8'))
        if manifest.get('source') != str(source) or manifest.get('version') != 1:
            raise ValueError('Build copy belongs to another source or marker version')
    elif destination.exists() and any(destination.iterdir()):
        raise ValueError('Refusing to initialize a nonempty directory without a workspace marker')
    old = manifest['files']
    files, _ = project_files(source)
    checker = Checker(known_credentials(source))
    payloads = {}
    for name in files:
        if private_path(name) or name.name == MARKER:
            continue
        path = safe_path(source, name)
        checker.file(path, name.as_posix())
        payloads[name.as_posix()] = (path.read_bytes(), stat.S_IMODE(path.stat().st_mode))
    if checker.failures:
        raise ValueError('Source credential verification failed; run scripts/verify_share.py --project-only')
    hashes = {name: hashlib.sha256(data).hexdigest() for name, (data, _) in payloads.items()}
    # Preflight all changes before writing anything. Retrying an interrupted sync
    # accepts either the previous managed hash or the current source hash.
    changed, removed = [], []
    for name in sorted(set(old) | set(hashes)):
        if private_path(Path(name)) or name == MARKER:
            raise ValueError('Private path in managed file manifest')
        target = safe_path(destination, name)
        if target.exists() and not target.is_file():
            raise ValueError('Managed file replaced by a directory: ' + name)
        current = digest(target)
        if current is not None and current not in (old.get(name), hashes.get(name)):
            raise ValueError('Local edit or unmanaged file conflict; copy changes to source first: ' + name)
        if name in hashes and current != hashes[name]:
            changed.append(name)
        elif name not in hashes and current is not None:
            removed.append(name)
    if not check:
        destination.mkdir(parents=True, exist_ok=True)
        # Persist ownership first so a first-sync interruption is safely retryable.
        if not marker.exists():
            atomic_write(marker, json.dumps({'version': 1, 'source': str(source), 'files': {}}, indent=2).encode())
        for name in changed:
            data, mode = payloads[name]
            atomic_write(safe_path(destination, name), data, mode)
        for name in removed:
            safe_path(destination, name).unlink()
        atomic_write(marker, json.dumps({'version': 1, 'source': str(source), 'files': hashes}, indent=2).encode())
    return len(hashes), len(changed), len(removed)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = Path(__file__).resolve().parents[1]
    parser.add_argument('--destination', type=Path, default=source.with_name(source.name + '-build'))
    parser.add_argument('--check', action='store_true', help='Report pending sync without writing')
    args = parser.parse_args()
    try:
        count, changed, removed = sync(source, args.destination, args.check)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f'Sync stopped: {error}\n')
    print(f'{"CHECK" if args.check else "SYNC"}: {count} source files; {changed} copies, {removed} removals. Private data preserved.')
    print(args.destination.resolve())
    return int(args.check and bool(changed or removed))


if __name__ == '__main__':
    raise SystemExit(main())
