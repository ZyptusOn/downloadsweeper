"""Run with workflow_regression.py: PDF evidence must be usable by the actual Agent tools."""
from pathlib import Path
from urllib.request import Request,urlopen
import copy,json,os,time,uuid
from pdf_fixture import pdf_bytes
BASE=os.environ['DS_TEST_URL']
def get(path):return json.load(urlopen(BASE+path,timeout=10))
boot=get('/api/bootstrap')
assert boot['config']['llm']['endpoint']=='http://127.0.0.1:3190/v1'
def post(action,task=None,**args):
    body={'action':action,**args}
    if task:body.update(task_id=task['id'],revision=task['revision'])
    return json.load(urlopen(Request(BASE+'/api/action',data=json.dumps(body).encode(),headers={'Content-Type':'application/json','x-ds-token':boot['token']}),timeout=10))
def wait(result):
    deadline=time.monotonic()+30
    while time.monotonic()<deadline:
        state=get('/api/bootstrap')
        if not state['job']:
            assert state['last_job']['status']=='completed',state['last_job']
            return get('/api/tasks/'+result['job']['task_id'])
        time.sleep(.04)
    raise AssertionError('timeout')
try:
    root=(Path('artifacts')/('pdf-agent-'+uuid.uuid4().hex[:8])).resolve();root.mkdir()
    (root/'course.pdf').write_bytes(pdf_bytes());(root/'private.csv').write_text('not authorized')
    cfg=copy.deepcopy(boot['config']);cfg['llm']['model']='classification-evidence-fixture'
    for tier,vision in [('image',True),('filename_only',True),('content_slice',False)]:
        cfg['llm']['multimodal']=vision;post('config',config=cfg)
        task=wait(post('scan',post('create',root=str(root),mode='organize')));task=post('advance',task)
        task=post('permissions',task,permissions={'default':'none','content_slice_bytes':64,'rules':[{'extensions':['pdf'],'tier':tier}]})
        task=post('advance',task)
        task=post('tree',task,nodes=[{'id':'docs','name':'文档','parent':'root','rule_type':'simple','extensions':['pdf']},
            {'id':'course','name':'课程资料','parent':'docs','rule_type':'complex','note':'PDF课程资料'}])
        task=wait(post('advance',task));start=len(Path('artifacts/mock-requests.jsonl').read_text(encoding='utf-8').splitlines())
        task=wait(post('plan_ai',task))
        wire=Path('artifacts/mock-requests.jsonl').read_text(encoding='utf-8').splitlines()[start:]
        requests=[json.loads(line)['body'] for line in wire]
        serialized=json.dumps(requests)
        assert 'private.csv' not in serialized
        assert len(task['calls'])==(2 if vision and tier=='image' else 1)
        assert ('data:image/jpeg' in serialized)==(vision and tier=='image')
        if vision and tier=='image':
            values=[json.loads(m['content']) for r in requests for m in r['messages'] if m['role']=='tool']
            assert any(f.get('sampled_pages')==[1,2,4] and f.get('image_index')==1 for v in values for f in v.get('files',[]))
            assert 'pdf_pages' in serialized
        assert task['classification']['completed']==1 and not task['pending_calls']
        assert (root/'course.pdf').read_bytes()==pdf_bytes()
    print('PASS PDF Agent tool selection, actual pages/image mapping, image-only permission, disabled vision and private-name filtering')
finally:post('config',config=boot['config'])
