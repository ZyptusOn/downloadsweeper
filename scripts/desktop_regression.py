"""Desktop HTTP lifecycle in an isolated loopback test server, never a real desktop."""
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import hashlib
import json
import os
import time
import uuid

BASE = os.environ['DS_TEST_URL']
assert BASE.startswith('http://127.0.0.1:')
def get(path):
    return json.load(urlopen(BASE + path, timeout=10))
boot = get('/api/bootstrap')
assert boot['config']['llm']['endpoint'] == 'http://127.0.0.1:3190/v1'
assert 'default_desktop_root' in boot['config']

def post(action, task=None, **args):
    data = {'action': action, **args}
    if task:
        data.update(task_id=task['id'], revision=task['revision'])
    return json.load(urlopen(Request(BASE + '/api/action', data=json.dumps(data).encode(),
        headers={'Content-Type': 'application/json', 'x-ds-token': boot['token']}), timeout=10))

def wait(result):
    job = result['job']
    for _ in range(500):
        state = get('/api/bootstrap')
        if not state['job']:
            assert state['last_job']['id'] == job['id'] and state['last_job']['status'] == 'completed', state['last_job']
            return get('/api/tasks/' + job['task_id'])
        time.sleep(.03)
    raise AssertionError('Job timed out')

def blocked(action, task, **args):
    try:
        post(action, task, **args)
    except HTTPError as error:
        assert error.code == 400
        return
    raise AssertionError('Missing safety gate: ' + action)

root = Path(__file__).resolve().parents[1] / 'artifacts' / ('desktop-http-' + uuid.uuid4().hex[:8])
(root / '项目/内部').mkdir(parents=True)
for name in ['待办.txt', '周报.docx', '资料.pdf', '截图.png', '网站.url', '~$周报.docx', '项目/内部/private.txt']:
    (root / name).write_text('synthetic fixture ' + name, encoding='utf-8')
def snapshot():
    return {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()}
before = snapshot()
task = post('create', root=str(root), mode='desktop')
blocked('desktop_plan', task)
task = wait(post('scan', task))
assert all(not e['parent'] for e in task['entries'])
blocked('directory', task, id='项目', **{'class': 'normal'})
task = post('advance', task)
assert task['phase'] == 1
task = post('advance', task)
assert task['phase'] == 2
task = wait(post('advance', task))
assert task['phase'] == 3
task = post('advance', task)
assert task['phase'] == 4 and len(task['operations']) == 4 and not task['calls']
assert len(task['retained']) == 3 and snapshot() == before
blocked('execute', task)
blocked('advance', task)
task = post('back', task, phase=0)
assert task['scanned'] and not task['operations']
task = wait(post('desktop_plan', task))
selected = [op['id'] for op in task['operations'] if op['source'] != '周报.docx']
task = post('review', task, selected=selected, reviewed=True)
task = post('advance', task)
task = wait(post('execute', task))
assert task['status'] == 'completed' and (root / '周报.docx').exists()
assert (root / '项目/内部/private.txt').exists() and (root / '网站.url').exists()
assert len([o for o in task['operations'] if o['status'] == 'done']) == 3
task = wait(post('rollback', task))
assert task['status'] == 'rolled_back' and snapshot() == before
assert not task['calls'] and not task['pending_calls']
print('PASS: desktop shallow scan, protected folders, six-stage review, selected moves and lossless restore; no LLM calls')
