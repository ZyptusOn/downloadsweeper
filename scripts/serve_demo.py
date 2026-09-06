"""Launch an isolated GUI + loopback mock model. No live API configuration is read.

Keep this terminal open; Ctrl+C stops only the child service and mock it owns.
The GUI remains the normal application; mock responses are deterministic fixtures.
"""
from pathlib import Path
from http.server import ThreadingHTTPServer
import argparse
import json
import os
import subprocess
import threading
import time
import uuid
from mock_llm import Mock

ROOT = Path(__file__).resolve().parents[1]


class DemoMock(Mock):
    def do_GET(self):
        if self.path.rstrip('/') == '/v1/models':
            self.respond(json.dumps({'object': 'list', 'data': [
                {'id': 'parallel-progress-fixture', 'object': 'model', 'context_length': 128000},
                {'id': 'classification-evidence-fixture', 'object': 'model', 'context_length': 128000},
            ]}).encode())
        else:
            self.send_error(404)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path, default=ROOT / 'target/release' / ('ds-web.exe' if os.name == 'nt' else 'ds-web'))
    args = parser.parse_args()
    exe = args.exe.resolve()
    if not exe.is_file(): parser.error('Build ds-web --release first, or pass --exe')
    runtime = ROOT / 'artifacts' / ('gui-demo-' + uuid.uuid4().hex[:8])
    runtime.mkdir(parents=True)
    mock = ThreadingHTTPServer(('127.0.0.1', 0), DemoMock)
    mock.daemon_threads = True
    endpoint = f'http://127.0.0.1:{mock.server_port}/v1'
    config = runtime / 'config.toml'
    config.write_text('token_budget = 500000\n[llm]\nendpoint = ' + json.dumps(endpoint) + '\nmodel = "parallel-progress-fixture"\napi_key_env = "DS_DEMO_EMPTY"\ncontext_length = 128000\nmax_output_tokens = 8192\nparallel_requests = 3\nmultimodal = true\n[permissions]\ndefault = "filename_only"\ncontent_slice_bytes = 4096\n', encoding='utf-8')
    env = {k: v for k, v in os.environ.items() if 'API_KEY' not in k and not k.startswith('DS_MODEL_KEY_') and k not in ('DS_DEMO_EMPTY', 'DS_CONFIG', 'DS_DATA_DIR')}
    os.chdir(runtime)
    threading.Thread(target=mock.serve_forever, daemon=True).start()
    log = (runtime / 'server.log').open('w', encoding='utf-8')
    child = None
    try:
        child = subprocess.Popen([str(exe), '--port', '0', '--config', str(config), '--data-dir', str(runtime / 'data')], cwd=runtime, env=env, stdout=log, stderr=log, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
        url = None
        for _ in range(150):
            if child.poll() is not None: raise RuntimeError('GUI startup failed; see ' + str(runtime / 'server.log'))
            for word in (runtime / 'server.log').read_text(encoding='utf-8').split():
                if word.startswith('http://127.0.0.1:'): url = word
            if url: break
            time.sleep(.1)
        if not url: raise RuntimeError('GUI startup timed out')
        print('GUI: ' + url, flush=True)
        print('Mock Endpoint: ' + endpoint + ' (no Key, no external requests)', flush=True)
        print('Runtime: ' + str(runtime), flush=True)
        print('Open the GUI, choose a generated downloads/desktop folder. Ctrl+C stops the demo.', flush=True)
        child.wait()
    except KeyboardInterrupt:
        pass
    finally:
        if child is not None and child.poll() is None:
            child.terminate()
            child.wait(timeout=10)
        mock.shutdown()
        mock.server_close()
        log.close()


if __name__ == '__main__':
    main()
