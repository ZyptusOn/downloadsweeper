"""Whole-folder Agent classification, privacy and lossless moves with a local mock only."""
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import copy, hashlib, json, os, time, uuid

base = os.environ['DS_TEST_URL']
def get(path): return json.load(urlopen(base+path,timeout=10))
boot = get('/api/bootstrap')
assert boot['config']['llm']['endpoint']=='http://127.0.0.1:3190/v1'
def post(action, task=None, **args):
    data={'action':action,**args}
    if task: data.update(task_id=task['id'],revision=task['revision'])
    return json.load(urlopen(Request(base+'/api/action',data=json.dumps(data).encode(),headers={'Content-Type':'application/json','x-ds-token':boot['token']}),timeout=10))
def wait(result, status='completed'):
    for _ in range(600):
        state=get('/api/bootstrap')
        if not state['job']:
            assert state['last_job']['id']==result['job']['id'] and state['last_job']['status']==status,state['last_job']
            return get('/api/tasks/'+result['job']['task_id'])
        time.sleep(.05)
    raise AssertionError('Timed out')
def snapshot(root): return {p.relative_to(root).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()}
project=Path(__file__).resolve().parents[1]
wire=project/'artifacts/mock-requests.jsonl'
cfg=copy.deepcopy(boot['config']);cfg['llm']['model']='directory-classification-fixture'
try:
    post('config',config=cfg)
    for mode in ['desktop','organize']:
        root=project/'artifacts'/('atomic-folder-'+mode+'-'+uuid.uuid4().hex[:8])
        name='xx中学八年级2026春期末考成绩'
        folder=root/name;folder.mkdir(parents=True)
        allowed=['按班级总分平均分.xlsx','按班级各科平均分.xlsx','全校学生成绩排名.xlsx']
        for n in allowed: (folder/n).write_text('Synthetic spreadsheet fixture',encoding='utf-8')
        (folder/'DENIED_PRIVATE.docx').write_text('PRIVATE_CONTENT_MUST_NOT_LEAK',encoding='utf-8')
        (root/'shortcut.url').write_text('URL=http://invalid.local',encoding='utf-8')
        before=snapshot(root)
        task=wait(post('scan',post('create',root=str(root),mode=mode)))
        task=post('advance',task)
        task=post('directory',task,id=name,**{'class':'atomic'})
        task=post('permissions',task,permissions={'default':'filename_only','rules':[{'extensions':['docx','url'],'tier':'none'}],'content_slice_bytes':128})
        task=post('advance',task)
        task=wait(post('advance',task))
        offset=wire.stat().st_size
        task=wait(post('plan_ai',task))
        assert task['classification']['completed']==1
        ops=[o for o in task['operations'] if o['source']==name]
        assert len(ops)==1 and ops[0]['kind']=='directory' and ops[0]['destination'].endswith('/'+name)
        assert '文档' in ops[0]['destination'].split('/')[0]
        assert not any(o['source'].startswith(name+'/') for o in task['operations'])
        if mode=='desktop': assert ops[0]['directory_manifest']
        assert snapshot(root)==before
        with wire.open('rb') as stream:
            stream.seek(offset); bodies=[json.loads(line)['body'] for line in stream if line.strip()]
        payload=json.dumps(bodies,ensure_ascii=False)
        assert all(n in payload for n in allowed)
        assert 'DENIED_PRIVATE' not in payload and 'PRIVATE_CONTENT_MUST_NOT_LEAK' not in payload
        assert any(m['role']=='tool' for r in bodies for m in r['messages'])
        task=post('advance',task)
        task=post('review',task,selected=[ops[0]['id']],reviewed=True)
        task=post('advance',task)
        if mode=='desktop':
            # Editing an internal file does not change the outer directory timestamp.
            (folder/allowed[0]).write_text('Changed fixture after review',encoding='utf-8')
            task=wait(post('execute',task),'failed')
            assert folder.is_dir() and not (root/ops[0]['destination']).exists()
            before=snapshot(root)
            task=post('back',task,phase=3)
            task=wait(post('plan_ai',task))
            ops=[o for o in task['operations'] if o['source']==name]
            task=post('advance',task)
            task=post('review',task,selected=[ops[0]['id']],reviewed=True)
            task=post('advance',task)
        task=wait(post('execute',task))
        assert not folder.exists() and (root/ops[0]['destination']/allowed[0]).is_file()
        task=wait(post('rollback',task))
        assert snapshot(root)==before
    print('PASS desktop/download whole-folder Agent tools, child privacy, reviewed directory manifest, whole move and lossless undo')

    # Missing format categories may be proposed as new top-level nodes in both modes.
    cfg['llm']['model']='new-top-level-fixture';post('config',config=cfg)
    for mode in ['desktop','organize']:
        root=project/'artifacts'/('new-top-'+mode+'-'+uuid.uuid4().hex[:8]);root.mkdir()
        for i in range(6): (root/f'meeting-{i}.mp3').write_text('fixture',encoding='utf-8')
        task=wait(post('scan',post('create',root=str(root),mode=mode)))
        task=post('advance',post('advance',task))
        # Simulate a user template missing the audio branch.
        task=post('tree',task,nodes=[n for n in task['nodes'] if n['id'] not in ['audio']])
        original=copy.deepcopy(task['nodes'])
        task=wait(post('suggest_tree',task,message='为大量音频补齐一级分类和录音子目录'))
        assert task['nodes']==original
        proposal=task['proposal'];assert len(proposal['changes'])==3
        child=next(c for c in proposal['changes'] if c['target']=='new-recordings')
        try:
            post('proposal',task,proposal_id=proposal['id'],scene='tree',ids=[child['id']])
            raise AssertionError('Dependent child merge accepted without its parent')
        except HTTPError as e: assert e.code==400
        assert get('/api/tasks/'+task['id'])['nodes']==original
        task=post('proposal',task,proposal_id=proposal['id'],scene='tree',ids=[c['id'] for c in proposal['changes']])
        assert next(n for n in task['nodes'] if n['id']=='new-music')['parent']=='root'
    print('PASS desktop/download proposed new first-level audio category, explicit merge and atomic parent-dependency rejection')
finally:
    post('config',config=boot['config'])
