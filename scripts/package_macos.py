"""Build and verify an Apple Silicon browser package locally on a Mac. No signing credentials used."""
from pathlib import Path
import argparse, datetime, hashlib, json, os, platform, shutil, stat, subprocess, sys, tomllib, zipfile

ROOT = Path(__file__).resolve().parents[1]

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--include-cli', action='store_true')
    args = parser.parse_args()
    if sys.platform != 'darwin' or platform.machine() != 'arm64':
        parser.error('Run this script on an Apple Silicon Mac using an arm64 Python/Rust toolchain.')
    def run(*command): subprocess.run(command, cwd=ROOT, check=True)
    run('cargo', 'run', '--locked', '-p', 'ds-dev', '--', 'verify-share', '--project-only')
    target = 'aarch64-apple-darwin'
    command = ['cargo', 'build', '--release', '--locked', '--target', target, '-p', 'ds-web']
    if args.include_cli: command += ['-p', 'ds-cli']
    run(*command)
    version = tomllib.loads((ROOT/'Cargo.toml').read_text())['workspace']['package']['version']
    name = f'DownloadSweeper-{version}-macos-arm64-' + datetime.datetime.now().strftime('%Y%m%d-%H%M%S-%f')
    folder = ROOT/'dist'/name; folder.mkdir(parents=True, exist_ok=False)
    for executable in ['ds-web'] + (['ds'] if args.include_cli else []):
        shutil.copy2(ROOT/'target'/target/'release'/executable, folder/executable)
        (folder/executable).chmod(0o755)
    for source, output in [('config.example.toml','config.example.toml'),('.env.example','.env.example'),
                           ('docs/MACOS.md','README.md'),('docs/MACOS.md','MACOS.md'),('docs/AGENT_RUNTIME.md','AGENT_RUNTIME.md'),
                           ('docs/MULTIMODAL.md','MULTIMODAL.md'),('docs/PRICING.md','PRICING.md'),
                           ('docs/ARCHIVES_AND_CLEANUP.md','ARCHIVES_AND_CLEANUP.md'),
                           ('docs/CHECKPOINTS.md','CHECKPOINTS.md'),
                           ('LICENSE','LICENSE'),('frontend/vendor/LICENSES.txt','THIRD_PARTY_LICENSES.txt')]:
        shutil.copy2(ROOT/source, folder/output)
    launcher = folder/'Start.command'
    launcher.write_text('#!/bin/sh\nset -eu\ncd "$(dirname "$0")"\nexec ./ds-web --open "$@"\n', encoding='utf-8')
    launcher.chmod(0o755)
    run('cargo', 'run', '--locked', '-p', 'ds-dev', '--', 'verify-share', '--package', str(folder))
    archive = Path(str(folder) + '.zip')
    with zipfile.ZipFile(archive, 'x', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as package:
        for file in sorted(folder.iterdir()):
            package.write(file, str(Path(name)/file.name))
    run('cargo', 'run', '--locked', '-p', 'ds-dev', '--', 'verify-share', '--package', str(archive))
    with zipfile.ZipFile(archive) as package:
        for executable in ['Start.command','ds-web'] + (['ds'] if args.include_cli else []):
            entry = package.getinfo(f'{name}/{executable}')
            assert entry.create_system == 3 and stat.S_IMODE(entry.external_attr >> 16) & 0o111
    manifest = {'target':target, 'archive':archive.name, 'bytes':archive.stat().st_size,
                'sha256':hashlib.sha256(archive.read_bytes()).hexdigest(), 'notarized':False,
                'files':[{'name':p.name,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(folder.iterdir())]}
    (ROOT/'dist'/f'{name}.manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
    print(archive)

if __name__ == '__main__': main()
