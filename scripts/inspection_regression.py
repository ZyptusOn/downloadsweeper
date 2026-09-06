"""Local HTTP regression: overview -> bounded type batches -> proposal, with resume.
Requires mock_llm.py on 3190 and ds-web on 3189 with isolated test data/config.
"""
import copy
import json
import time
import uuid
from pathlib import Path
from urllib.request import Request, urlopen

import os
BASE = os.environ.get('DS_TEST_URL', 'http://127.0.0.1:3189')
def get(path):
    return json.load(urlopen(BASE + path, timeout=10))
boot = get('/api/bootstrap')
original_config = boot['config']
assert original_config['llm']['endpoint'].startswith('http://127.0.0.1:3190'), 'Local fixture only'
def post(action, task=None, **args):
    data = {'action':action, **args}
    if task:
        data.update(task_id=task['id'], revision=task['revision'])
    return json.load(urlopen(Request(BASE+'/api/action', data=json.dumps(data).encode(),
                                    headers={'Content-Type':'application/json','x-ds-token':boot['token']}), timeout=10))
def wait(result, expected='completed'):
    for _ in range(500):
        state = get('/api/bootstrap')
        if not state['job']:
            assert state['last_job']['id'] == result['job']['id']
            assert state['last_job']['status'] == expected, state['last_job']
            return get('/api/tasks/'+result['job']['task_id'])
        time.sleep(.02)
    raise AssertionError('Job timeout')
log_path = Path('artifacts/mock-requests.jsonl')
start_line = len(log_path.read_text(encoding='utf-8').splitlines()) if log_path.exists() else 0
def requests():
    return [json.loads(line)['body'] for line in log_path.read_text(encoding='utf-8').splitlines()[start_line:]]
def context(request):
    content = request['messages'][-1]['content']
    try:
        return json.loads(content[0]['text'] if isinstance(content,list) else content)
    except (ValueError, KeyError):
        return {}

root = (Path('artifacts') / ('inspection-fixture-'+uuid.uuid4().hex[:8])).resolve()
root.mkdir(parents=True)
for i in range(55):
    (root/f'observe_{i:02}.txt').write_text(('允许的文本切片' * 800) if i == 0 else '允许读取的内容', encoding='utf-8')
for i in range(25):
    (root/f'video_{i:02}.mp4').write_text('dummy video', encoding='utf-8')
(root/'private_NEVER_SEND.xlsx').write_text('NEVER_SEND_TABLE_CONTENT', encoding='utf-8')
(root/'name_only.docx').write_text('NEVER_SEND_DOCX_CONTENT', encoding='utf-8')
(root/'Portable').mkdir()
(root/'Portable/editor.exe').write_text('program',encoding='utf-8')
(root/'Portable/NEVER_SEND_PROTECTED.txt').write_text('protected',encoding='utf-8')

try:
    config = copy.deepcopy(original_config)
    config['token_budget'] = None
    config['llm'].update(model='inspection-pause-fixture',context_length=24000, parallel_requests=1)
    post('config', config=config)
    task = post('create', root=str(root))
    task = wait(post('scan', task))
    task = post('advance', task)
    task = post('permissions', task, permissions={'default':'filename_only','content_slice_bytes':4096,'rules':[
        {'extensions':['xlsx','@folder'],'tier':'none'}, {'extensions':['txt'],'tier':'content_slice'}, {'extensions':['mp4'],'tier':'metadata'}]})
    task = post('advance', task)
    nodes = copy.deepcopy(task['nodes'])
    job = post('suggest_tree', task, scene='tree', message='先查看总览，再分类型检查并建议结构')
    for _ in range(500):
        current = get('/api/tasks/'+task['id'])
        pending = get('/api/bootstrap')['job']
        if current.get('inspection') and sum(g['inspected'] for g in current['inspection']['groups']) > 0 and pending and '第 2 批' in pending['message']:
            break
        time.sleep(.02)
    else:
        raise AssertionError('Did not reach second batch')
    completed_before = sum(g['inspected'] for g in current['inspection']['groups'])
    post('cancel', job_id=job['job']['id'])
    task = wait(job, 'paused')
    assert task['inspection']['status'] == 'paused'
    assert sum(g['inspected'] for g in task['inspection']['groups']) == completed_before
    assert task['nodes'] == nodes and task['proposal'] is None
    assert task['calls'] and all(c['usage']['prompt_tokens'] == 321 for c in task['calls'])
    print('PASS cancellation saves completed batches and exact usage')

    recorded = len(requests())
    config['token_budget'] = sum(c['usage']['prompt_tokens'] + c['usage']['completion_tokens'] for c in task['calls']) + 1
    post('config',config=config)
    task = wait(post('suggest_tree',task,scene='tree',message='预算不足时不得继续发送'),'failed')
    assert len(requests()) == recorded
    assert sum(g['inspected'] for g in task['inspection']['groups']) == completed_before
    config['token_budget'] = None
    print('PASS budget stops before another request and preserves the checkpoint')

    config['llm']['model'] = 'inspection-pause-fixture'
    post('config', config=config)
    task = wait(post('suggest_tree', task, scene='tree', message='继续分析并建议结构'))
    state = task['inspection']
    assert state['status'] == 'complete'
    assert sum(g['inspected'] for g in state['groups']) == 81
    assert sum(g['withheld'] for g in state['groups']) == 2 and state['protected_files'] == 2
    assert task['proposal'] and task['nodes'] == nodes
    contexts = [context(r) for r in requests()]
    overviews = [c for c in contexts if c.get('stage') == 'overview']
    batches = [c for c in contexts if c.get('stage') == 'inspect_batch']
    assert len(overviews) == 1
    assert len([c for c in batches if c['type_id']=='text' and c['batch']==1]) == 1
    assert all(0 < len(c['files']) <= 24 for c in batches)
    assert all(len(json.dumps(c['files'],ensure_ascii=False,separators=(',',':')).encode()) <= 8192 for c in batches)
    assert all(len(f.get('text_excerpt','').encode()) <= 1024 for c in batches for f in c['files'])
    assert 'observe_00.txt' not in json.dumps(overviews)
    all_requests = json.dumps(requests(),ensure_ascii=False)
    for denied in ['private_NEVER_SEND.xlsx','NEVER_SEND_TABLE_CONTENT','NEVER_SEND_DOCX_CONTENT','NEVER_SEND_PROTECTED.txt']:
        assert denied not in all_requests, denied
    final_request = requests()[-1]
    assert 'file_inspection' in json.dumps(final_request)
    assert 'observe_00.txt' not in json.dumps(final_request)
    print('PASS overview privacy, bounded batches, type coverage, summary-only final context and explicit proposal')

    calls = len(task['calls'])
    task['nodes'][0]['note'] = '编辑树后仍可使用文件观察结果'
    task = post('tree',task,nodes=task['nodes'])
    task = wait(post('suggest_tree',task,scene='tree',message='根据已完成分析再给建议'))
    assert len(task['calls']) == calls + 1
    assert len([context(r) for r in requests() if context(r).get('stage')=='overview']) == 1
    print('PASS completed observations reused after tree edits')
    task = post('back',task,phase=1)
    assert task['inspection'] is None
    permissions = task['permissions']
    permissions['rules'][1]['tier'] = 'none'
    task = post('permissions',task,permissions=permissions)
    task = post('advance',task)
    offset = len(requests())
    task = wait(post('suggest_tree',task,scene='tree',message='按更新后的权限重新检查'))
    assert sum(g['inspected'] for g in task['inspection']['groups']) == 26
    assert 'observe_00.txt' not in json.dumps(requests()[offset:])
    print('PASS permission changes invalidate prior observations and filter the next inspection')
    Path('artifacts/inspection-regression.json').write_text(json.dumps({'task_id':task['id'],'inspected':81,'checks':5}),encoding='utf-8')
finally:
    post('config',config=original_config)
