"""Planning-stage regression against an isolated web server and local mock LLM.

Run mock_llm.py on 3190 and ds-web on 3189 with its own data/config first.
Only creates temporary fixture files; never executes any plan.
"""
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import copy
import hashlib
import json
import os
import time
import uuid

BASE = os.environ.get('DS_TEST_URL', 'http://127.0.0.1:3189')


def get(path):
    return json.load(urlopen(BASE + path, timeout=10))


bootstrap = get('/api/bootstrap')
assert not bootstrap['job'], 'Wait for the isolated server to be idle'
assert bootstrap['config']['llm']['endpoint'] == 'http://127.0.0.1:3190/v1', 'Local mock only'
token = bootstrap['token']


def post(action, task=None, **kwargs):
    data = {'action': action, **kwargs}
    if task:
        data.update(task_id=task['id'], revision=task['revision'])
    request = Request(BASE + '/api/action', data=json.dumps(data).encode(),
                      headers={'Content-Type': 'application/json', 'x-ds-token': token})
    return json.load(urlopen(request, timeout=10))


def wait(result, status='completed'):
    job = result['job']
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        state = get('/api/bootstrap')
        if not state['job']:
            last = state['last_job']
            assert last['id'] == job['id'] and last['status'] == status, last
            return get('/api/tasks/' + job['task_id'])
        time.sleep(.04)
    raise AssertionError('Timed out waiting for ' + job['kind'])


def blocked(action, task):
    try:
        post(action, task)
    except HTTPError as error:
        assert error.code == 400, error.read().decode()
        return
    raise AssertionError('Stage gate missing: ' + action)


def enter_planning(root):
    task = post('create', root=str(root), mode='organize')
    task = wait(post('scan', task))
    task = post('advance', task)
    task = post('advance', task)
    result = post('advance', task)
    assert result['job']['kind'] == 'plan_rules', result
    task = wait(result)
    assert task['phase'] == 3 and task['status'] == 'planned'
    assert task['plan_source'] == 'rules' and not task['calls']
    return task


root = (Path(__file__).resolve().parents[1] / 'artifacts' / ('planning-fixture-' + uuid.uuid4().hex[:8]))
root.mkdir()
for name in ['movie.mp4', 'lecture.mp4', 'notes.txt', 'Portable/app.exe']:
    path = root / name
    path.parent.mkdir(exist_ok=True)
    path.write_text('local planning fixture: ' + name, encoding='utf-8')


def fingerprint():
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob('*') if p.is_file()}


before = fingerprint()
initial_config = bootstrap['config']
try:
    cfg = copy.deepcopy(initial_config)
    cfg['llm']['model'] = 'local-test'
    cfg['token_budget'] = 100000
    post('config', config=cfg)
    task = enter_planning(root)
    original_ids = {o['source']: o['id'] for o in task['operations']}
    assert original_ids and any(o['source'] == 'Portable' for o in task['operations'])
    blocked('execute', task)

    task = wait(post('plan_ai', task))
    assert task['phase'] == 3 and task['plan_source'] == 'ai'
    assert {o['source']: o['id'] for o in task['operations']} == original_ids
    assert any(o['destination'].startswith('视频/电影/') for o in task['operations'])
    assert task['calls'] and not task['reviewed']
    task = post('advance', task)
    blocked('advance', task)
    selected = [o['id'] for o in task['operations']][1:]
    task = post('review', task, selected=selected, reviewed=True)
    task = wait(post('plan_ai', task))
    assert task['phase'] == 4 and not task['reviewed']
    assert [o['id'] for o in task['operations'] if o['selected']] == selected

    draft = copy.deepcopy(task['operations'])
    task = post('back', task, phase=3)
    assert task['operations'] == draft and task['plan_source'] == 'ai'
    task = post('advance', task)
    assert task['operations'] == draft

    # An unsuccessful refinement must preserve the existing draft and IDs.
    calls_before = len(task['calls'])
    cfg['llm']['model'] = 'invalid-json-fixture'
    post('config', config=cfg)
    task = wait(post('plan_ai', task), 'failed')
    assert task['phase'] == 4 and task['operations'] == draft
    assert task['plan_source'] == 'ai' and len(task['calls']) == calls_before + 2

    cfg['llm']['model'] = 'planning-pause-fixture'
    post('config', config=cfg)
    pending = post('plan_ai', task)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        active = get('/api/bootstrap')['job']
        if active and '等待模型' in active['message']:
            assert active['total'] > 0, 'Model heartbeat lost file progress'
            break
        time.sleep(.03)
    else:
        raise AssertionError('Never reached the cancellable model call')
    post('cancel', job_id=pending['job']['id'])
    task = wait(pending, 'paused')
    assert task['operations'] == draft and task['phase'] == 4
    blocked('execute', task)
    assert fingerprint() == before, 'Planning changed fixture files'

    task = post('back', task, phase=2)
    assert not task['operations'] and task['plan_source'] is None
    cfg['llm']['model'] = 'local-test'
    post('config', config=cfg)
    task = wait(post('advance', task))
    assert task['phase'] == 3 and task['plan_source'] == 'rules'

    empty_root = root / 'Empty'
    empty_root.mkdir()
    empty = enter_planning(empty_root)
    assert not empty['operations']
    empty = post('advance', empty)
    assert empty['phase'] == 4 and not empty['reviewed']

    report = {'task_id': task['id'], 'empty_task_id': empty['id'], 'files_unchanged': True,
              'passed': ['automatic_rules', 'optional_refinement', 'explicit_review',
                         'refine_from_review', 'selection_preserved', 'back_preserves_plan',
                         'failure_preserves_plan_and_usage', 'cancellation_preserves_plan',
                         'upstream_invalidates_plan', 'empty_plan_review', 'execution_gate']}
    (root.parent / 'planning-regression.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report))
finally:
    post('config', config=initial_config)
