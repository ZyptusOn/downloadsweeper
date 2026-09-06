"""Real HTTP/SSE and filesystem regression suite, using only its own fixture directory."""
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import json
import time
import uuid
import hashlib
import os

BASE = os.environ.get('DS_TEST_URL', 'http://127.0.0.1:3189')
def get(path):
    return json.load(urlopen(BASE + path, timeout=10))
bootstrap = get('/api/bootstrap')
token = bootstrap['token']
assert bootstrap['config']['llm']['endpoint'].startswith('http://127.0.0.1:3190'), 'Use the local test config only'
def post(action, context=None, **kwargs):
    data = {'action': action, **kwargs}
    if context:
        data.update(task_id=context['id'], revision=context['revision'])
    request = Request(BASE + '/api/action', data=json.dumps(data).encode(),
                      headers={'Content-Type': 'application/json', 'x-ds-token': token})
    try:
        return json.load(urlopen(request, timeout=10))
    except HTTPError as error:
        raise AssertionError(error.read().decode()) from error
def wait_job(result):
    job = result['job']
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        state = get('/api/bootstrap')
        if not state['job']:
            last = state['last_job']
            assert last['id'] == job['id']
            assert last['status'] == 'completed', last
            return get('/api/tasks/' + job['task_id'])
        time.sleep(.05)
    raise AssertionError('Job timed out')
def expected_error(action, task=None, **kwargs):
    try:
        post(action, task, **kwargs)
    except AssertionError:
        return
    raise AssertionError('Unsafe action accepted: ' + action)

root = (Path('artifacts') / ('http-fixture-' + uuid.uuid4().hex[:8])).resolve()
root.mkdir(parents=True)
for rel, content in {'readme.txt':'permitted text', 'private.xlsx':'denied table',
                     'clip.mp4':'fixture video', 'cache.log':'fixture temporary log', 'Portable/editor.exe':'fixture software',
                     'Portable/config.json':'{}', '文档/工作资料/existing.pdf':'existing'}.items():
    file = root / rel
    file.parent.mkdir(parents=True, exist_ok=True)
    file.write_text(content, encoding='utf-8')
before = {str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()}
task = post('create', root=str(root), mode='organize')
expected_error('advance', task)
task = wait_job(post('scan', task))
assert task['phase'] == 0 and task['scanned']
task = post('advance', task)
permissions = task['permissions']
permissions['rules'] = [{'extensions':['xlsx'], 'tier':'none'}, {'extensions':['txt'], 'tier':'content_slice'}]
task = post('permissions', task, permissions=permissions)
snapshot = task['entries']
task = post('directory', task, id='Portable', **{'class':'atomic'})
assert task['entries'] == snapshot
task = post('advance', task)
task = wait_job(post('chat', task, scene='tree', message='给当前目录一个可合并的建议'))
proposal = task['proposal']
assert proposal and proposal['changes']
expected_error('proposal', task, proposal_id=proposal['id'], scene='permissions', ids=[proposal['changes'][0]['id']])
task = post('proposal', task, proposal_id=proposal['id'], scene='tree', ids=[proposal['changes'][0]['id']])
assert task['nodes'][0]['note'] == '保留原有分类，减少不必要的移动。'
assert task['calls'][0]['usage'] == {'prompt_tokens':321,'completion_tokens':45,'cached_input_tokens':0,'cache_write_tokens':0,'cache_write_1h_tokens':0,'cache_details_known':False}
task = wait_job(post('advance', task))
assert task['phase'] == 3 and task['plan_source'] == 'rules'
task = wait_job(post('plan_ai', task))
assert task['phase'] == 3 and task['plan_source'] == 'ai'
task = post('advance', task)
assert task['phase'] == 4 and task['operations']
assert not any(o['source'].startswith('Portable/') for o in task['operations'])
assert not any('existing.pdf' in o['source'] for o in task['operations'])
assert all(not o['destination'].startswith('视频') for o in task['operations'] if o['source'].endswith('.xlsx'))
expected_error('execute', task)
task = post('review', task, selected=[o['id'] for o in task['operations']], reviewed=True)
task = post('advance', task)
task = wait_job(post('execute', task))
assert task['status'] == 'completed'
task = wait_job(post('cleanup_ai', task))
assert any(c['source']=='ai' for c in task['cleanup'])
task = wait_job(post('rollback', task))
assert task['status'] == 'rolled_back'
after = {str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()}
assert before == after
events = get('/api/tasks/' + task['id'] + '/trajectory')
assert any(e['kind']=='move_intent' for e in events)
assert any(e['kind']=='restore_done' for e in events)
assert task['messages']
imported = post('import', task=task)
assert imported['id'] != task['id'] and not imported['scanned']
assert imported['messages'] == task['messages'] and not imported['operations']
imported = wait_job(post('scan', imported))
assert imported['nodes'][0]['note'] == task['nodes'][0]['note']

# Local-only HTTP guard, token protection and no filesystem asset exposure.
def forbidden(path, headers, data=None, code=403):
    try:
        urlopen(Request(BASE+path, data=data, headers=headers),timeout=3)
    except HTTPError as error:
        assert error.code == code, error.code
        return
    raise AssertionError('Expected HTTP rejection')
forbidden('/api/bootstrap', {'Origin':'https://unrelated.example'})
forbidden('/api/bootstrap', {'Host':'unrelated.example'})
forbidden('/api/action', {'Content-Type':'application/json'},b'{"action":"create"}')
forbidden('/config.toml', {}, code=404)

initial_config = get('/api/bootstrap')['config']
try:
    # Exact prices use returned usage, and secrets never appear in bootstrap.
    cfg = json.loads(json.dumps(initial_config))
    cfg['llm']['pricing'] = {'mode':'manual','input_per_1k_usd':.001,'output_per_1k_usd':.002}
    cfg['search'] = {'enabled':True,'endpoint':'http://127.0.0.1:3190/search','api_key':'fixture-search-key'}
    saved = post('config', config=cfg)
    assert 'api_key' not in saved['llm'] and 'api_key' not in saved['search']
    rename_task = post('create', root=str(root), mode='rename')
    rename_task = wait_job(post('scan', rename_task))
    rename_task = post('advance',rename_task)
    rename_permissions = rename_task['permissions']
    rename_permissions['rules'] = [{'extensions':['xlsx'],'tier':'none'}]
    rename_task = post('permissions',rename_task,permissions=rename_permissions)
    rename_task = post('advance',rename_task)
    rename_task = post('rename_scope',rename_task,extensions=['txt','xlsx'],web_search=True)
    rename_task = post('advance',rename_task)
    rename_task = wait_job(post('rename',rename_task))
    assert len(rename_task['operations'])==1 and rename_task['operations'][0]['source']=='readme.txt'
    assert len(rename_task['search_calls'])==1 and rename_task['search_calls'][0]['query']=='readme.txt'
    assert abs(rename_task['calls'][0]['cost_usd']-.000411)<1e-12
    assert 'https://example.com/fixture' in rename_task['operations'][0]['reason']
    rename_task = post('review',rename_task,selected=[o['id'] for o in rename_task['operations']],reviewed=True)
    rename_task = post('advance',rename_task)
    rename_task = wait_job(post('execute',rename_task))
    rename_task = wait_job(post('rollback',rename_task))
    assert before == {str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()}

    # Subscribe to actual SSE events, then cancel a deliberately slow model request.
    stream = urlopen(BASE+'/api/events',timeout=5)
    pending = post('chat', imported, scene='scan', message='slow-fixture')
    saw_progress = False
    for _ in range(12):
        line = stream.readline().decode().strip()
        if line.startswith('data:'):
            event = json.loads(line[5:])
            if event['type']=='progress':
                saw_progress = True
                break
    assert saw_progress
    started = time.monotonic()
    post('cancel',job_id=pending['job']['id'])
    while get('/api/bootstrap')['job']:
        assert time.monotonic()-started<3, 'Cancellation was not responsive'
        time.sleep(.03)
    assert get('/api/bootstrap')['last_job']['status']=='paused'
    stream.close()
    imported = get('/api/tasks/'+imported['id'])

    # The budget rejects a request before sending; missing usage is an error, never zero.
    cfg['token_budget']=1
    post('config',config=cfg)
    original_calls = len(imported['calls'])
    started_job = post('test_connection', imported)
    while get('/api/bootstrap')['job']:
        time.sleep(.03)
    failed = get('/api/bootstrap')['last_job']
    assert failed['status']=='failed' and '预算' in failed['error']
    assert len(get('/api/tasks/'+imported['id'])['calls'])==original_calls
    cfg['token_budget']=30000
    cfg['llm']['model']='missing-usage-fixture'
    post('config',config=cfg)
    imported = get('/api/tasks/'+imported['id'])
    post('test_connection', imported)
    while get('/api/bootstrap')['job']:
        time.sleep(.03)
    failed = get('/api/bootstrap')['last_job']
    assert failed['status']=='failed' and 'token' in failed['error']
    assert len(get('/api/tasks/'+imported['id'])['calls'])==original_calls
finally:
    post('config',config=initial_config,clear_search_key=True)

print('PASS: staged HTTP workflow, permissions, scene diffs, AI planning, search-assisted rename, cleanup review, exact usage/cost, SSE/cancel, budget/missing-usage rejection, execute/rollback hashes, context import, same-origin/token guard')
