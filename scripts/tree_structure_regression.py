"""Local HTTP coverage of model graph creation, partial edits, dependency checks and merge.
Requires mock_llm.py on 3190 and isolated ds-web/config on 3189. No file moves.
"""
import copy
import json
import time
import uuid
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError

import os
BASE = os.environ.get('DS_TEST_URL', 'http://127.0.0.1:3189')
def get(path):
    return json.load(urlopen(BASE + path, timeout=10))
boot = get('/api/bootstrap')
original = boot['config']
assert original['llm']['endpoint'].startswith('http://127.0.0.1:3190'), 'Local fixture only'
def post(action, task=None, **args):
    data = {'action':action, **args}
    if task:
        data.update(task_id=task['id'], revision=task['revision'])
    return json.load(urlopen(Request(BASE+'/api/action', data=json.dumps(data).encode(),
        headers={'Content-Type':'application/json','x-ds-token':boot['token']}), timeout=10))
def wait(result, expected='completed'):
    for _ in range(300):
        state = get('/api/bootstrap')
        if not state['job']:
            assert state['last_job']['id'] == result['job']['id']
            assert state['last_job']['status'] == expected, state['last_job']
            return get('/api/tasks/'+result['job']['task_id'])
        time.sleep(.03)
    raise AssertionError('Job timeout')

root = (Path('artifacts') / ('tree-structure-fixture-'+uuid.uuid4().hex[:8])).resolve()
root.mkdir(parents=True)
(root/'school.mp4').write_bytes(b'fixture video')
(root/'reference.pdf').write_bytes(b'fixture reference')
config = copy.deepcopy(original)
config['token_budget'] = None
config['llm'].update(model='tree-structure-fixture', context_length=65536)
try:
    post('config',config=config)
    task = post('create',root=str(root),mode='organize')
    task = wait(post('scan',task))
    task = post('advance',task)
    task = post('advance',task)
    nodes = copy.deepcopy(task['nodes'])
    movie = next(n for n in nodes if n['name']=='电影')
    movie['examples'] = ['school.mp4']
    movie['position'] = [812, 364]
    task = post('tree',task,nodes=nodes)
    original_nodes = copy.deepcopy(task['nodes'])
    task = wait(post('suggest_tree',task,scene='tree',message='根据摘要新增子目录、调整节点描述'))
    assert task['nodes'] == original_nodes
    proposal = task['proposal']
    assert len(proposal['changes']) == 6
    new = [c for c in proposal['changes'] if c['before'] is None]
    assert len(new) == 2
    modified = next(c for c in proposal['changes'] if c['target']==movie['id'])['after']
    assert modified['name']=='影视长片' and modified['parent']==movie['parent']
    assert modified['examples']==movie['examples'] and modified['position']==movie['position']
    assert modified['rule_type']==movie['rule_type']
    print('PASS new nested nodes and partial note/rename edits become reviewable changes; examples/position preserved')

    child = next(c for c in new if c['target']=='fixture-campus')
    assert child['after']['name']=='校园记录／学业'
    assert '全角字符' in proposal['message']
    print('PASS generated folder separators are normalized and disclosed in the review message')
    try:
        post('proposal',task,proposal_id=proposal['id'],scene='tree',ids=[child['id']])
        raise AssertionError('Child-only merge should be rejected')
    except HTTPError as error:
        assert '父节点' in json.load(error)['error']
    assert get('/api/tasks/'+task['id'])['nodes'] == original_nodes
    print('PASS incomplete parent dependencies fail atomically with an actionable message')

    task = post('proposal',task,proposal_id=proposal['id'],scene='tree',ids=[c['id'] for c in proposal['changes']])
    by_id = {n['id']:n for n in task['nodes']}
    assert by_id['fixture-campus']['parent']=='fixture-topics'
    assert by_id['fixture-topics']['parent']==movie['parent']
    assert by_id[movie['id']]['name']=='影视长片'
    assert not any(n['name']=='番剧' for n in task['nodes'])
    assert next(n for n in task['nodes'] if n['name']=='剪辑素材')['parent']=='fixture-topics'
    assert task['proposal'] is None and task['operations']==[]
    assert get('/api/tasks/'+task['id'])['nodes']==task['nodes']
    assert (root/'school.mp4').read_bytes()==b'fixture video'
    print('PASS creation, nested links, note edits, rename, reparenting and deletion persist after explicit merge')

    # Restore only graph rules in this isolated task; observations remain cached.
    task = post('tree',task,nodes=original_nodes)
    config['llm']['model']='invalid-tree-fixture'
    post('config',config=config)
    task = wait(post('chat',task,scene='tree',message='生成无效父节点测试'), 'failed')
    assert task['nodes']==original_nodes and task['proposal'] is None
    assert '有效目录结构' in get('/api/bootstrap')['last_job']['error']
    print('PASS invalid graph rejected before publishing a proposal; usage remains recorded')

    config['llm']['model']='tree-structure-fixture'
    post('config',config=config)
    task = wait(post('suggest_tree',task,scene='tree',message='生成供浏览器审查的完整目录改动'))
    request = json.loads(Path('artifacts/mock-requests.jsonl').read_text(encoding='utf-8').splitlines()[-1])['body']
    assert 'file_inspection' in request['messages'][1]['content']
    assert any('多层分类结构' in m['content'] for m in request['messages'] if m['role']=='system')
    assert not any(m['role']=='assistant' for m in request['messages'])
    print('PASS structure-design instructions use cached findings without anchoring to prior assistant suggestions')
    Path('artifacts/tree-structure-regression.json').write_text(json.dumps({'task_id':task['id'],'checks':6}),encoding='utf-8')
finally:
    post('config',config=original)
