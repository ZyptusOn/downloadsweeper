"""Recreating the default Photos node must yield a reviewable edit, not a failed job."""
from pathlib import Path
from urllib.request import Request, urlopen
import copy, json, os, time, uuid

base=os.environ['DS_TEST_URL']
def get(path): return json.load(urlopen(base+path,timeout=10))
boot=get('/api/bootstrap')
assert boot['config']['llm']['endpoint']=='http://127.0.0.1:3190/v1'
def post(action,task=None,**args):
    data={'action':action,**args}
    if task: data.update(task_id=task['id'],revision=task['revision'])
    return json.load(urlopen(Request(base+'/api/action',data=json.dumps(data).encode(),headers={'Content-Type':'application/json','x-ds-token':boot['token']}),timeout=10))
def wait(result):
    for _ in range(400):
        state=get('/api/bootstrap')
        if not state['job']:
            assert state['last_job']['id']==result['job']['id'] and state['last_job']['status']=='completed',state['last_job']
            return get('/api/tasks/'+result['job']['task_id'])
        time.sleep(.03)
    raise AssertionError('Timed out')
cfg=copy.deepcopy(boot['config']);cfg['llm']['model']='duplicate-template-fixture'
try:
    post('config',config=cfg)
    for mode in ['desktop','organize']:
        root=Path(__file__).resolve().parents[1]/'artifacts'/('duplicate-photo-'+mode+'-'+uuid.uuid4().hex[:8])
        root.mkdir();(root/'sample.jpg').write_text('Synthetic name-only fixture',encoding='utf-8')
        task=wait(post('scan',post('create',root=str(root),mode=mode)))
        task=post('advance',task)
        task=post('permissions',task,permissions={'default':'filename_only','rules':[],'content_slice_bytes':0})
        task=post('advance',task)
        nodes=copy.deepcopy(task['nodes']);photo=next(n for n in nodes if n['name']=='照片')
        photo['examples']=['sample.jpg'];photo['position']=[800,240]
        task=post('tree',task,nodes=nodes)
        task=wait(post('suggest_tree',task,message='按照默认模板细化照片分类'))
        assert task['nodes']==nodes,'Suggestions must not change the user tree before approval'
        proposal=task['proposal'];assert proposal and '复用同名节点' in proposal['message']
        assert len(proposal['changes'])==2
        edit=next(c for c in proposal['changes'] if c['target']==photo['id'])
        assert edit['before']==photo and edit['after']['examples']==photo['examples']
        assert edit['after']['position']==photo['position']
        child=next(c for c in proposal['changes'] if c['target']=='photo-travel')
        assert child['after']['parent']==photo['id']
        assert all(c['target']!='duplicate-photo' for c in proposal['changes'])
        assert len(task['calls'])==3,'Overview, inspection and proposal only; no paid repair request'
        trajectory=get('/api/tasks/'+task['id']+'/trajectory')
        assert any(e['kind']=='ai_proposal_nodes_reused' for e in trajectory)
        task=post('proposal',task,proposal_id=proposal['id'],scene='tree',ids=[c['id'] for c in proposal['changes']])
        assert len([n for n in task['nodes'] if n['parent']==photo['parent'] and n['name']=='照片'])==1
        assert next(n for n in task['nodes'] if n['id']=='photo-travel')['parent']==photo['id']
        assert (root/'sample.jpg').read_text(encoding='utf-8')=='Synthetic name-only fixture'
    print('PASS desktop/download duplicate default Photos reconciliation, preserved examples/positions, child references, explicit merge and no extra API calls')
finally:
    post('config',config=boot['config'])
