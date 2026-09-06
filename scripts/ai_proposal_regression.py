"""Regression for reasoning truncation, empty replies and proposal merging.

Run mock_llm.py on 3190 and ds-web with an isolated test data/config on 3189.
This script does not call remote services or move any files.
"""
from pathlib import Path
from urllib.request import Request, urlopen
import copy
import json
import time
import uuid

import os
BASE = os.environ.get('DS_TEST_URL', 'http://127.0.0.1:3189')

def get(path):
    return json.load(urlopen(BASE + path, timeout=10))

boot = get('/api/bootstrap')
assert boot['config']['llm']['endpoint'].startswith('http://127.0.0.1:3190'), 'Local fixture config required'
original = boot['config']

def post(action, task=None, **args):
    data = {'action': action, **args}
    if task:
        data.update(task_id=task['id'], revision=task['revision'])
    return json.load(urlopen(Request(BASE + '/api/action', data=json.dumps(data).encode(),
                                    headers={'Content-Type': 'application/json', 'x-ds-token': boot['token']}), timeout=10))

def wait(result):
    for _ in range(200):
        state = get('/api/bootstrap')
        if not state['job']:
            assert state['last_job']['id'] == result['job']['id']
            return state['last_job'], get('/api/tasks/' + result['job']['task_id'])
        time.sleep(.05)
    raise AssertionError('Job timed out')

config = copy.deepcopy(original)
config['token_budget'] = None
config['llm']['max_output_tokens'] = 16384
config['llm']['context_length'] = 65536
config['llm']['thinking_mode'] = True
root = (Path('artifacts') / ('ai-proposal-fixture-' + uuid.uuid4().hex[:8])).resolve()
root.mkdir(parents=True)
(root / 'readme.txt').write_text('local fixture', encoding='utf-8')
task = post('create', root=str(root), mode='organize')
job, task = wait(post('scan', task))
assert job['status'] == 'completed'
task = post('advance', task)
task = post('advance', task)
before = copy.deepcopy(task['nodes'])

try:
    # These were previously accepted as successful empty suggestions.
    for model, expected in [('truncated-reasoning-fixture', '回答被截断'),
                            ('truncated-json-fixture', '回答被截断'),
                            ('empty-answer-fixture', '没有返回最终回答'),
                            ('invalid-json-fixture', '改动 JSON')]:
        config['llm']['model'] = model
        post('config', config=config)
        call_count = len(task['calls'])
        job, task = wait(post('chat', task, scene='tree', message='生成可合并的结构建议'))
        assert job['status'] == 'failed' and expected in job['error'], job
        assert task['nodes'] == before and task['proposal'] is None
        assert not any(m['role'] == 'assistant' for m in task['messages'])
        assert len(task['calls']) == call_count + 1
        assert task['calls'][-1]['max_output_tokens'] == 16384
        assert task['calls'][-1]['usage']['completion_tokens'] == (16384 if model.startswith('truncated-') else 45)
        print('PASS', model)

    config['llm']['model'] = 'deepseek-fixture'
    post('config', config=config)
    job, task = wait(post('chat', task, scene='tree', message='生成可合并的结构建议'))
    assert job['status'] == 'completed' and '1 项建议' in job['message'], job
    proposal = task['proposal']
    assert proposal and task['nodes'] == before
    assert task['calls'][-1]['finish_reason'] == 'stop'
    merged = post('proposal', task, proposal_id=proposal['id'], scene='tree', ids=[c['id'] for c in proposal['changes']])
    assert merged['nodes'][0]['note'] == '保留原有分类，减少不必要的移动。'
    assert merged['proposal'] is None and not merged['operations']
    assert (root / 'readme.txt').read_text(encoding='utf-8') == 'local fixture'
    print('PASS structured proposal generated, explicitly merged and persisted')
    Path('artifacts/ai-proposal-regression.json').write_text(json.dumps({'task_id': merged['id'], 'checks': 5}), encoding='utf-8')
finally:
    post('config', config=original)
