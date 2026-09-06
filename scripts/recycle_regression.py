"""Explicit native recycle/undo integration; only self-created fixture files are recycled.

Uses a private server/config, no model calls. Never empties the system recycle bin.
"""
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import hashlib
import json
import os
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
base = ROOT / 'artifacts' / ('recycle-http-' + uuid.uuid4().hex[:8])
files = base / 'files'
files.mkdir(parents=True)
for name in ['临时缓存.log', 'keep.txt']:
    (files / name).write_text('synthetic recycle roundtrip ' + name, encoding='utf-8')
before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in files.iterdir()}
config = base / 'config.toml'
config.write_text('[llm]\nendpoint="http://127.0.0.1:3190/v1"\nmodel="local-test"\napi_key_env="DS_TEST_EMPTY"\n', encoding='utf-8')
env = {k:v for k,v in os.environ.items() if 'API_KEY' not in k and not k.startswith('DS_')}
binary = ROOT / 'target/debug' / ('ds-web.exe' if os.name == 'nt' else 'ds-web')
process = None
log = None
task = None

def get(path):
    return json.load(urlopen(url + path, timeout=20))

def start():
    global process, log, url, token
    log_path = base / ('server-' + uuid.uuid4().hex[:6] + '.log')
    log = log_path.open('w', encoding='utf-8')
    process = subprocess.Popen([str(binary), '--port', '0', '--config', str(config), '--data-dir', str(base/'data')],
        cwd=ROOT, env=env, stdout=log, stderr=log, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    for _ in range(150):
        urls = [w.rstrip('/') for w in log_path.read_text(encoding='utf-8').split() if w.startswith('http://127.0.0.1:')]
        if urls:
            url = urls[-1]
            token = get('/api/bootstrap')['token']
            return
        time.sleep(.1)
    raise AssertionError('Server failed to start')

def stop():
    if process and process.poll() is None:
        process.terminate()
        process.wait(timeout=10)
    if log:
        log.close()

def post(action, current=None, **args):
    data = {'action':action, **args}
    if current:
        data.update(task_id=current['id'], revision=current['revision'])
    return json.load(urlopen(Request(url + '/api/action', data=json.dumps(data).encode(),
        headers={'Content-Type':'application/json', 'x-ds-token':token}), timeout=20))

def wait(result, status='completed'):
    for _ in range(600):
        state = get('/api/bootstrap')
        if not state['job']:
            job = state['last_job']
            assert job['id'] == result['job']['id'] and job['status'] == status, job
            return get('/api/tasks/' + result['job']['task_id'])
        time.sleep(.05)
    raise AssertionError('Job timed out')

try:
    start()
    assert get('/api/bootstrap')['config']['recycle_supported'], 'Native roundtrip needs Windows or macOS'
    task = post('create', root=str(files), mode='desktop')
    task = wait(post('scan', task))
    task = wait(post('desktop_plan', task))
    task = post('review', task, selected=[o['id'] for o in task['operations']], reviewed=True)
    task = post('advance', task)
    task = wait(post('execute', task))
    candidate = next(e for e in task['cleanup'] if e['original_id'] == '临时缓存.log')
    try:
        post('cleanup_trash', task, selected=['临时缓存.log'])
        raise AssertionError('Confirmation gate missing')
    except HTTPError as error:
        assert error.code == 400
    task = wait(post('cleanup_trash', task, selected=['临时缓存.log'], confirmed=True))
    assert task['recycled'][-1]['status'] == 'trashed'
    assert not (files / candidate['path']).exists()
    batch = task['recycled'][-1]['batch']
    task = wait(post('rollback', task), status='failed')
    assert task['status'] == 'completed'
    stop()
    start()
    task = get('/api/tasks/' + task['id'])
    assert task['recycled'][-1]['batch'] == batch
    task = wait(post('cleanup_restore', task, batch=batch))
    assert all(r['status'] == 'restored' for r in task['recycled'])
    assert (files / candidate['path']).is_file()
    task = wait(post('rollback', task))
    assert task['status'] == 'rolled_back' and not task['calls']
    assert before == {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in files.iterdir() if p.is_file()}
    (base / 'verification.json').write_text(json.dumps({'passed':True, 'task_id':task['id'], 'checks':['explicit selection and confirmation', 'native recycle', 'persisted undo after restart', 'organization rollback ordering', 'lossless restoration', 'zero model calls']}, indent=2), encoding='utf-8')
    print('PASS: native HTTP recycle, restart, one-click undo, ordering and unchanged content; no model calls')
finally:
    # On failure, attempt to undo only the batches belonging to our fixture task.
    if task and process and process.poll() is None:
        current = get('/api/tasks/' + task['id'])
        for batch in dict.fromkeys(r['batch'] for r in current.get('recycled', []) if r['status'] != 'restored'):
            try:
                current = wait(post('cleanup_restore', current, batch=batch))
            except Exception:
                print('Fixture recovery still pending; preserve test data at ' + str(base))
    stop()
