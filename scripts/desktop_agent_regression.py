"""Desktop six-stage Agent integration using synthetic files and the local mock."""
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import copy
import hashlib
import json
import os
import time
import uuid

base = os.environ['DS_TEST_URL']
assert base.startswith('http://127.0.0.1:')

def get(path):
    return json.load(urlopen(base + path, timeout=15))

boot = get('/api/bootstrap')
assert boot['config']['llm']['endpoint'] == 'http://127.0.0.1:3190/v1'

def post(action, task=None, **args):
    data = {'action': action, **args}
    if task:
        data.update(task_id=task['id'], revision=task['revision'])
    return json.load(urlopen(Request(base + '/api/action', data=json.dumps(data).encode(),
        headers={'Content-Type': 'application/json', 'x-ds-token': boot['token']}), timeout=15))

def wait(result):
    for _ in range(600):
        state = get('/api/bootstrap')
        if not state['job']:
            assert state['last_job']['id'] == result['job']['id'] and state['last_job']['status'] == 'completed', state['last_job']
            return get('/api/tasks/' + result['job']['task_id'])
        time.sleep(.05)
    raise AssertionError('Agent job timed out')

project = Path(__file__).resolve().parents[1]
root = project / 'artifacts' / ('desktop-agent-' + uuid.uuid4().hex[:8])
(root / 'Protected').mkdir(parents=True)
for name in ['brief.txt', 'reference.txt', 'shortcut.url', '.hidden.txt', '~$locked.txt', 'Protected/private.txt']:
    (root / name).write_text('Synthetic desktop project notes ' + name, encoding='utf-8')

def snapshot():
    return {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()}

before = snapshot()
cfg = copy.deepcopy(boot['config'])
cfg['llm']['model'] = 'classification-evidence-fixture'
wire_path = project / 'artifacts/mock-requests.jsonl'
offset = wire_path.stat().st_size if wire_path.exists() else 0
try:
    post('config', config=cfg)
    # The untouched desktop template must reach the Agent without manually adding nodes.
    default_task = wait(post('scan', post('create', root=str(root), mode='desktop')))
    default_task = post('advance', default_task)
    default_task = post('permissions', default_task, permissions={'default':'filename_only', 'rules':[], 'content_slice_bytes':128})
    default_task = post('advance', default_task)
    default_nodes = copy.deepcopy(default_task['nodes'])
    assert default_task['classification_readiness']['eligible_files'] == 3
    default_task = wait(post('advance', default_task))
    default_task = wait(post('plan_ai', default_task))
    assert default_task['classification']['completed'] == 3 and default_task['calls']
    assert default_task['nodes'] == default_nodes and snapshot() == before

    # Old/simple-only tasks must explain why nothing is eligible, without a fake AI success.
    legacy = post('back', default_task, phase=1)
    legacy = post('permissions', legacy, permissions={'default':'filename_only','rules':[{'extensions':['@folder'],'tier':'none'}],'content_slice_bytes':128})
    legacy = post('advance', legacy)
    legacy = post('tree', legacy, nodes=[n for n in default_nodes if n['rule_type']=='simple'])
    legacy = wait(post('advance', legacy))
    assert legacy['classification_readiness']['no_semantic_rule'] == 2
    unchanged = copy.deepcopy(legacy)
    try:
        post('plan_ai', legacy)
        raise AssertionError('Empty AI classification was accepted')
    except HTTPError as error:
        assert error.code == 400 and '未调用 AI' in json.load(error)['error']
    assert get('/api/tasks/' + legacy['id']) == unchanged
    assert legacy['plan_source'] == 'rules' and legacy['classification'] is None

    offset = wire_path.stat().st_size
    task = wait(post('scan', post('create', root=str(root), mode='desktop')))
    assert all(not e['parent'] for e in task['entries'])
    task = post('advance', task)
    assert task['phase'] == 1
    assert len(boot['config']['permission_presets']) >= 14
    snapshot_ids = [e['id'] for e in task['entries']]
    task = post('directory', task, id='Protected', **{'class':'container'})
    assert [e['id'] for e in task['entries']] == snapshot_ids
    task = post('permissions', task, permissions={'default': 'filename_only', 'content_slice_bytes': 128,
        'rules': [{'category':'text', 'extensions': ['txt','customtext'], 'tier': 'content_slice'}]})
    task = get('/api/tasks/' + task['id'])
    assert task['permissions']['rules'][0]['category'] == 'text'
    assert task['permissions']['rules'][0]['extensions'] == ['txt','customtext']
    task = post('advance', task)
    assert task['phase'] == 2
    task = wait(post('suggest_tree', task, message='Suggest desktop project categories using the local template'))
    assert task['inspection']['status'] == 'complete' and task['proposal']['changes']
    proposal = task['proposal']
    task = post('proposal', task, proposal_id=proposal['id'], scene='tree', ids=[c['id'] for c in proposal['changes']])
    nodes = copy.deepcopy(task['nodes'])
    top = next(n for n in nodes if 'txt' in n['extensions'])
    top['name'] = 'Desktop Projects'
    top['mapping'] = 'Protected'
    # This test explicitly chooses a single custom semantic destination for text files.
    nodes = [n for n in nodes if not n['id'].startswith('desktop-text-semantic-')]
    nodes.append({'id': 'project-notes', 'parent': top['id'], 'name': 'Notes', 'rule_type': 'complex',
        'extensions': [], 'note': 'Project briefs and references', 'examples': ['reference.txt'], 'mapping': None, 'position': None})
    nodes.append({'id': 'unused', 'parent': None, 'name': 'Unused', 'rule_type': 'simple',
        'extensions': [], 'note': '', 'examples': [], 'mapping': None, 'position': [-200, 400]})
    task = post('tree', task, nodes=nodes)
    task = get('/api/tasks/' + task['id'])
    assert next(n for n in task['nodes'] if n['id'] == 'unused')['position'] == [-200.0, 400.0]
    task = wait(post('advance', task))
    assert task['phase'] == 3 and task['nodes'] == nodes
    task = wait(post('plan_ai', task, batch_size=8))
    assert task['classification']['status'] == 'complete'
    assert task['classification']['completed'] == 2
    assert task['nodes'] == nodes and len(task['operations']) == 2
    assert all(o['destination'].startswith('Protected/Notes/') for o in task['operations'])
    assert task['calls'] and all(c['usage']['prompt_tokens'] > 0 for c in task['calls'])
    assert snapshot() == before, 'Agent planning must not move files'
    with wire_path.open('rb') as stream:
        stream.seek(offset)
        requests = [json.loads(line)['body'] for line in stream if line.strip()]
    assert any(m['role'] == 'tool' for r in requests for m in r['messages']), 'No Agent evidence/tool cycle observed'
    assert not any('private.txt' in json.dumps(r) for r in requests), 'Protected folder content leaked'
    task = post('advance', task)
    assert task['phase'] == 4
    task = post('review', task, selected=[o['id'] for o in task['operations']], reviewed=True)
    task = post('advance', task)
    task = wait(post('execute', task))
    assert (root / 'Protected/Notes/brief.txt').is_file()
    task = wait(post('rollback', task))
    assert snapshot() == before
    (root.parent / (root.name + '-verification.json')).write_text(json.dumps({'passed': True, 'task': task['id'],
        'checks': ['six stages', 'tree inspection and proposal merge', 'custom semantic tree and examples',
                   'persisted detached coordinates', 'Agent evidence tools and usage', 'reused folder contents preserved',
                   'permission category and custom extensions persisted', 'lossless undo']}), encoding='utf-8')
    print('PASS desktop complete Agent: tree suggestion/merge, editable semantic tree, evidence tools, usage and lossless undo')
finally:
    post('config', config=boot['config'])
