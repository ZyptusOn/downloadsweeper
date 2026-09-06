"""Native helper IPC/snapshot regression; own generated files, no live LLM or credentials."""
from pathlib import Path
import hashlib, json, os, subprocess, sys, uuid

ROOT = Path(__file__).resolve().parents[1]

def main():
    assert os.name == 'nt' or sys.platform == 'darwin'
    media = Path(os.environ['DS_TEST_MEDIA_BIN'])
    folder = ROOT/'artifacts'/('native-preview-'+uuid.uuid4().hex[:8]); folder.mkdir(parents=True)
    video = folder/'片段 测试.mp4'
    executable = ROOT/'target/debug'/('ds-web.exe' if os.name == 'nt' else 'ds-web')
    tool = media/('ffmpeg.exe' if os.name == 'nt' else 'ffmpeg')
    flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
    subprocess.run([str(tool),'-nostdin','-v','error','-f','lavfi','-i','testsrc2=size=320x240:rate=4:duration=6',
                    '-c:v','libx264','-threads','1','-g','1',str(video)],check=True,creationflags=flags)
    digest = hashlib.sha256(video.read_bytes()).hexdigest()
    # A native helper must not load even a deliberately invalid application config.
    (folder/'config.toml').write_text('invalid TOML [',encoding='utf-8')
    (folder/'.env').write_text('invalid environment fixture',encoding='utf-8')
    env = {'SystemRoot':os.environ['SystemRoot']} if os.name == 'nt' else {}
    def invoke(source, snapshot, path=video, stale=False):
        return subprocess.run([str(executable),'--native-video-preview',str(path),str(snapshot.st_size),
                str(snapshot.st_mtime_ns//1000000 + (1000 if stale else 0))],stdin=source,cwd=folder,env=env,
                capture_output=True,timeout=10,creationflags=flags)
    with video.open('rb') as source:
        snapshot = video.stat()
        result = invoke(source, snapshot)
        assert result.returncode == 0, 'Native helper failed on H264 fixture'
        visual = json.loads(result.stdout.splitlines()[-1])
        assert visual['info']['backend'] == ('windows_media' if os.name == 'nt' else 'avfoundation')
        assert len(visual['info']['sample_targets_seconds']) == 3
        assert visual['image']['mime'] == 'image/jpeg' and len(result.stdout) <= 3*1024*1024
        if os.name == 'nt': assert not result.stderr
        source.seek(0)
        assert invoke(source, snapshot, stale=True).returncode != 0
    assert hashlib.sha256(video.read_bytes()).hexdigest() == digest
    if sys.platform == 'darwin':
        # Prove AVFoundation uses the inherited file, not the replaced pathname.
        with video.open('rb') as source:
            snapshot = video.stat()
            original = folder/'original.mp4'; video.rename(original)
            video.write_bytes(b'not the authorized video')
            result = invoke(source, snapshot)
            assert result.returncode == 0
            assert json.loads(result.stdout.splitlines()[-1])['info']['backend']=='avfoundation'
            assert hashlib.sha256(original.read_bytes()).hexdigest()==digest
    print('PASS native helper: bounded JPEG, Unicode path, no config loading, stale snapshot refusal, unchanged source')

if __name__ == '__main__': main()
