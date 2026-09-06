"""Owned loopback integration: archives, context selection, cleanup privacy and batching."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import hashlib, json, math, os, subprocess, sys, threading, time, uuid
ROOT=Path(__file__).resolve().parents[1]
requests=[]
class Mock(BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append(body)
        if 'slow-archive-check' in body['messages'][-1]['content']:time.sleep(1)
        if '复核清理候选' in body['messages'][0]['content']:
            items=json.loads(body['messages'][-1]['content'])
            answer={'suggestions':[{'index':c['index'],'reason':'请确认安装包是否仍需要，保留唯一副本。'} for c in items]}
        else:answer={'message':'已记录你的整理偏好。','changes':[]}
        data=json.dumps({'choices':[{'message':{'role':'assistant','content':json.dumps(answer)},'finish_reason':'stop'}],
            'usage':{'prompt_tokens':300,'completion_tokens':40}}).encode()
        self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
mock=ThreadingHTTPServer(('127.0.0.1',0),Mock)
threading.Thread(target=mock.serve_forever,daemon=True).start()
directory=ROOT/'artifacts'/('archive-cleanup-'+uuid.uuid4().hex[:8]);directory.mkdir(parents=True)
root=directory/'Downloads';root.mkdir();(root/'Portable').mkdir()
for name in [*[f'installer-{i:02}.msi' for i in range(65)],'private.zip','Portable/app.exe','Portable/cache.log']:
    path=root/name;path.write_text('BODY_MUST_STAY_LOCAL',encoding='utf-8');old=time.time()-400*86400;os.utime(path,(old,old))
original={str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()}
config=directory/'config.toml'
config.write_text(f'[llm]\nendpoint="http://127.0.0.1:{mock.server_port}/v1"\nmodel="fixture-model"\napi_key_env="DS_TEST_EMPTY"\ncontext_length=128000\nmax_output_tokens=4096\n',encoding='utf-8')
env={k:v for k,v in os.environ.items() if 'API_KEY' not in k and not k.startswith('DS_MODEL_KEY_') and k!='DS_TEST_EMPTY'}
log=(directory/'server.log').open('w',encoding='utf-8')
process=subprocess.Popen([str(ROOT/'target/debug'/('ds-web.exe' if os.name=='nt' else 'ds-web')),'--port','0','--config',str(config),'--data-dir',str(directory/'data')],cwd=ROOT,env=env,stdout=log,stderr=log,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
try:
    base=None
    for _ in range(150):
        for word in (directory/'server.log').read_text(encoding='utf-8').split():
            if word.startswith('http://127.0.0.1:'):base=word.rstrip('/')
        if base:break
        time.sleep(.1)
    assert base,'server startup failed'
    def get(path):return json.load(urlopen(base+path,timeout=15))
    boot=get('/api/bootstrap')
    def post(action,current=None,**args):
        payload={'action':action,**args}
        if current:payload.update(task_id=current['id'],revision=current['revision'])
        return json.load(urlopen(Request(base+'/api/action',data=json.dumps(payload).encode(),headers={'Content-Type':'application/json','x-ds-token':boot['token']}),timeout=15))
    def wait(result):
        for _ in range(600):
            state=get('/api/bootstrap')
            if not state['job']:
                assert state['last_job']['id']==result['job']['id'] and state['last_job']['status']=='completed',state['last_job']
                return get('/api/tasks/'+result['job']['task_id'])
            time.sleep(.025)
        raise AssertionError('job timeout')
    task=post('create',root=str(root));task=wait(post('scan',task));task=post('advance',task)
    task=post('permissions',task,permissions={'default':'none','content_slice_bytes':0,'rules':[{'extensions':['msi'],'tier':'filename_only'}]})
    task=post('directory',task,id='Portable',**{'class':'atomic'})
    # Legacy JSON import remains available, safely clearing executable state.
    task['search_calls']=[{'roundtrip':{'large_integer':2**64-1,'whole_float':1.0,'negative_zero':-0.0}}]
    task['messages']=[{'role':'user' if i%2==0 else 'assistant','scene':'permissions','content':f'history-marker-{i:02}'} for i in range(40)]
    task=post('import',**{'task':task});task=wait(post('scan',task));task=post('advance',task)
    task=wait(post('chat',task,scene='permissions',message='请继续'))
    sent=json.dumps(requests[-1],ensure_ascii=False)
    assert 'history-marker-00' in sent and 'history-marker-39' in sent
    assert task['chat_context']['included']==40
    slow=post('chat',task,scene='permissions',message='slow-archive-check')
    try:get('/api/tasks/'+task['id']+'/archive');raise AssertionError('active archive accepted')
    except HTTPError as error:assert error.code==400
    task=wait(slow)
    print('PASS history beyond 8 turns and active-job archive consistency gate')
    task=post('advance',task);task=wait(post('advance',task));task=post('advance',task)
    task=post('review',task,selected=[o['id'] for o in task['operations']],reviewed=True)
    task=post('advance',task);task=wait(post('execute',task))
    assert task['status']=='completed'
    assert not any(c['original_id'].startswith('Portable/') for c in task['cleanup'])
    count=len(requests);task=wait(post('cleanup_ai',task));batches=requests[count:]
    assert len(batches)==math.ceil(65/32),len(batches)
    for request in batches:
        items=json.loads(request['messages'][-1]['content'])
        assert len(items)<=32 and all(set(c)=={'index','file'} and set(c['file'])=={'name','extension'} for c in items)
    wire=json.dumps(batches)
    assert 'private.zip' not in wire and 'BODY_MUST_STAY_LOCAL' not in wire and 'cache.log' not in wire
    assert sum(c['source']=='ai' for c in task['cleanup'])==65
    print('PASS seven-category candidates, package protection, filename-only privacy and bounded AI batches')
    archive=get('/api/tasks/'+task['id']+'/archive')
    contents=json.loads(archive['payload'])
    assert contents['task']==task and contents['trajectory']==get('/api/tasks/'+task['id']+'/trajectory')
    wire=json.loads(subprocess.check_output(['node','-e',"process.stdout.write(JSON.stringify(JSON.parse(require('fs').readFileSync(0,'utf8'))))"],input=json.dumps(archive).encode(),timeout=10))
    assert wire['payload']==archive['payload']
    assert contents['task']['search_calls'][0]['roundtrip']['large_integer']==2**64-1
    saved=post('import',**{'task':wire});archive_id=saved['archive_id']
    assert get('/api/archives/'+archive_id)==archive
    assert archive_id not in [t['id'] for t in get('/api/tasks')]
    bad=json.loads(json.dumps(archive));bad['payload']=bad['payload'].replace('history-marker-00','tampered')
    try:post('import',**{'task':bad});raise AssertionError('tampered archive accepted')
    except HTTPError as error:assert error.code==400
    resumed=post('resume_archive',archive_id=archive_id,root=str(root))
    assert not resumed['scanned'] and not resumed['operations'] and resumed['messages']==task['messages']
    assert get('/api/archives/'+archive_id)==archive
    # No content was changed by execution, cleanup, export, import or resume.
    assert sorted(hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file())==sorted(original.values())
    print('PASS archive exact task/trajectory roundtrip, tamper rejection and safe resume; file hashes unchanged')
    print('GUI fixture:',base,flush=True)
    print('Completed task:',task['id'],flush=True)
    if '--serve' in sys.argv:
        while True:time.sleep(1)
finally:
    process.terminate();process.wait(timeout=10);mock.shutdown();log.close()
