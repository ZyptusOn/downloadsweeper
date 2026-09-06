"""Local mock only: review edits, privacy, dependency rejection and real parallel progress."""
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import copy
import json
import os
import time
import uuid

BASE = os.environ.get('DS_TEST_URL', 'http://127.0.0.1:3189')
def get(path): return json.load(urlopen(BASE + path, timeout=10))
boot = get('/api/bootstrap')
assert boot['config']['llm']['endpoint'] == 'http://127.0.0.1:3190/v1'
def post(action, task=None, **args):
    data = dict(action=action, **args)
    if task: data.update(task_id=task['id'], revision=task['revision'])
    return json.load(urlopen(Request(BASE+'/api/action', data=json.dumps(data).encode(),
        headers={'Content-Type':'application/json', 'x-ds-token':boot['token']}), timeout=10))
observed = []
def wait(result, status='completed'):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        state = get('/api/bootstrap')
        if not state['job']:
            assert state['last_job']['id'] == result['job']['id'] and state['last_job']['status'] == status, state['last_job']
            return get('/api/tasks/' + result['job']['task_id'])
        if state['job'].get('parallel'): observed.append(state['job']['parallel'])
        time.sleep(.025)
    raise AssertionError('timeout')
cfg = copy.deepcopy(boot['config'])
cfg['token_budget'] = None
cfg['llm']['parallel_requests'] = 3
def model(name):
    cfg['llm']['model'] = name
    post('config', config=cfg)
def node(id, parent, name, exts=None):
    return dict(id=id, parent=parent, name=name, rule_type='simple' if exts else 'complex', extensions=exts or [], note='', examples=[], mapping=None)
def merge(task, ids=None, status='completed'):
    proposal=task['proposal']
    return wait(post('proposal',task,proposal_id=proposal['id'],scene='review',ids=ids if ids is not None else [c['id'] for c in proposal['changes']]),status)
try:
    for mode in ['desktop', 'organize']:
        root = (Path('artifacts') / ('review-'+mode+'-'+uuid.uuid4().hex[:8])).resolve()
        root.mkdir(parents=True)
        for name in ['clip.mp4','notes.txt','private.xlsx']: (root/name).write_text('fixture-'+name)
        task=wait(post('scan',post('create',root=str(root),mode=mode)))
        task=post('advance',task)
        task=post('permissions',task,permissions={'default':'filename_only','content_slice_bytes':64,'rules':[{'extensions':['xlsx'],'tier':'none'}]})
        task=post('advance',task)
        task=post('tree',task,nodes=[node('other','root','其它',['*']),node('docs','root','文档',['txt']),node('work','docs','工作')])
        task=wait(post('advance',task))
        model('local-test')
        task=wait(post('plan_ai',task,batch_size=2))
        task=post('advance',task)
        notes=next(o for o in task['operations'] if o['source']=='notes.txt')
        assert notes['destination']=='文档/工作/notes.txt'
        task=post('review',task,selected=[o['id'] for o in task['operations'] if o['source']!='notes.txt'],reviewed=True)
        before=copy.deepcopy(task['operations'])
        model('review-split-fixture')
        task=wait(post('chat',task,scene='review',message='把视频从其它移出来放到单独一级目录'))
        assert task['operations']==before and task['reviewed'] and task['proposal']
        calls=len(task['calls'])
        task=merge(task)
        assert task['phase']==4 and task['status']=='planned' and not task['reviewed']
        assert next(o for o in task['operations'] if o['source']=='clip.mp4')['destination']=='视频/clip.mp4'
        assert next(o for o in task['operations'] if o['source']=='notes.txt')==next(o for o in before if o['source']=='notes.txt')
        assert len(task['calls'])==calls and task['classification'] is None
        print('PASS',mode,'format split, unaffected AI results/deselections preserved, no extra model call')
        model('review-placement-fixture')
        task=wait(post('chat',task,scene='review',message='把notes放到新建笔记子目录'))
        before=copy.deepcopy(task['operations']); nodes=copy.deepcopy(task['nodes'])
        placement=next(c for c in task['proposal']['changes'] if c['kind']=='placement')
        task=merge(task,[placement['id']],status='failed')
        assert task['operations']==before and task['nodes']==nodes and task['proposal']
        task=merge(task)
        notes=next(o for o in task['operations'] if o['source']=='notes.txt')
        assert notes['destination']=='文档/笔记/notes.txt' and not notes['selected']
        model('review-keep-fixture')
        task=wait(post('chat',task,scene='review',message='clip保持原位'))
        task=merge(task)
        assert not any(o['source']=='clip.mp4' for o in task['operations'])
        assert any(r['source']=='clip.mp4' for r in task['retained'])
        model('review-invalid-fixture')
        before=copy.deepcopy(task['operations'])
        task=wait(post('chat',task,scene='review',message='invalid'),status='failed')
        assert task['operations']==before
        assert sorted(p.name for p in root.iterdir())==['clip.mp4','notes.txt','private.xlsx']
        for p in root.iterdir(): assert p.read_text()=='fixture-'+p.name
        print('PASS',mode,'placement dependencies, retained file, atomic rejection and no disk moves')
    # A semantic-only edit must also place files, transactionally and with resumable batches.
    root = (Path('artifacts') / ('review-semantic-'+uuid.uuid4().hex[:8])).resolve(); root.mkdir()
    for i in range(260): (root/f'note{i:03}.txt').write_text('lesson fixture')
    (root/'clip.mp4').write_text('video fixture')
    (root/'private.xlsx').write_text('private fixture')
    task=wait(post('scan',post('create',root=str(root),mode='desktop')))
    task=post('advance',task)
    task=post('permissions',task,permissions={'default':'filename_only','content_slice_bytes':64,'rules':[{'extensions':['xlsx'],'tier':'none'}]})
    task=post('advance',task)
    task=post('tree',task,nodes=[node('other','root','其它',['*']),node('docs','root','文档',['txt']),node('work','docs','工作')])
    task=wait(post('advance',task)); task=post('advance',task)
    task=post('review',task,selected=[o['id'] for o in task['operations'] if o['source']!='note000.txt'],reviewed=True)
    model('review-semantic-resume-fixture')
    task=wait(post('chat',task,scene='review',message='在文档下新建学习笔记，按内容归类'))
    before=copy.deepcopy(task['operations']); old_nodes=copy.deepcopy(task['nodes']); calls=len(task['calls'])
    pending=post('proposal',task,proposal_id=task['proposal']['id'],scene='review',ids=[c['id'] for c in task['proposal']['changes']])
    live=get('/api/tasks/'+task['id'])
    assert live['operations']==before and live['nodes']==old_nodes
    task=wait(pending,'failed')
    assert task['operations']==before and task['nodes']==old_nodes and task['proposal']
    run=task['review_classification']
    assert run['total']==260 and 0<run['completed']<260 and run['status']=='failed',run
    finished=sum(b['status']=='complete' for b in run['batches'])
    failed_calls=len(task['calls'])
    stopped=get('/api/bootstrap')['last_job']
    assert stopped['resumable']
    # A saved truncated response must not be silently charged again by automatic recovery.
    task=wait(post('resume_job',job_id=stopped['id']),'failed')
    assert len(task['calls'])==failed_calls and task['operations']==before
    task=merge(task)  # Explicit retry starts a new run while preserving valid completed batches.
    assert task['proposal'] is None and task['review_classification']['status']=='complete'
    assert len(task['calls'])-failed_calls==len(run['batches'])-finished
    assert all(o['destination']=='文档/学习笔记/'+o['source'] for o in task['operations'] if o['source'].endswith('.txt'))
    assert not next(o for o in task['operations'] if o['source']=='note000.txt')['selected']
    assert next(o for o in task['operations'] if o['source']=='clip.mp4')==next(o for o in before if o['source']=='clip.mp4')
    assert len(list(root.iterdir()))==262 and (root/'note000.txt').read_text()=='lesson fixture'
    wire=Path('artifacts/mock-requests.jsonl').read_text(encoding='utf-8').splitlines()
    semantic=[json.loads(line)['body'] for line in wire if 'review-semantic-resume-fixture' in line]
    classification=[r for r in semantic if r.get('tools')]
    assert classification and all('clip.mp4' not in json.dumps(r) and 'private.xlsx' not in json.dumps(r) for r in classification)
    print('PASS semantic-only review auto-planning, scoped calls, atomic failure, saved-batch reuse, deselections and no moves')

    # Confirm actual simultaneous workers and monotonically completed batches.
    root = (Path('artifacts') / ('parallel-'+uuid.uuid4().hex[:8])).resolve(); root.mkdir()
    for i in range(10): (root/f'clip{i}.mp4').write_text('fixture')
    task=wait(post('scan',post('create',root=str(root),mode='organize')))
    task=post('advance',task); task=post('advance',task); task=wait(post('advance',task))
    model('parallel-progress-fixture'); observed.clear()
    task=wait(post('plan_ai',task,batch_size=2))
    assert any(sum(b['status']=='running' for b in s['batches'])==3 and any(b['status']=='pending' for b in s['batches']) for s in observed)
    assert [s['completed_files'] for s in observed]==sorted(s['completed_files'] for s in observed)
    assert all(s['completed_files']==sum(b['files'] for b in s['batches'] if b['status']=='complete') for s in observed)
    assert task['classification']['completed']==10 and len(task['calls'])==5
    wire=Path('artifacts/mock-requests.jsonl').read_text(encoding='utf-8').splitlines()
    requests=[json.loads(line)['body'] for line in wire if 'review-split-fixture' in line]
    assert requests and all('private.xlsx' not in json.dumps(r) for r in requests)
    Path('artifacts/review-plan-regression.json').write_text(json.dumps({'task_id':task['id'],'parallel_samples':len(observed),'checks':'review edits, dependencies, privacy, preservation, concurrent batches'}))
    print('PASS three concurrent workers, queue/completion counts, monotonic progress and private filename withheld')
finally:
    post('config',config=boot['config'])
