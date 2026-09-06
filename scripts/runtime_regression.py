"""Isolated HTTP regression for bounded parallelism, checkpoints and reservation accounting."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import hashlib, json, os, subprocess, threading, time, uuid

ROOT = Path(__file__).resolve().parents[1]
class Mock(BaseHTTPRequestHandler):
    lock = threading.Lock()
    active = 0
    maximum = 0
    calls = []
    fail_once = True
    def log_message(self, *args): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        with self.lock:
            Mock.active += 1; Mock.maximum = max(Mock.maximum, Mock.active); Mock.calls.append(body)
        try:
            if body['model']=='runtime-denied':
                self.send_response(401);self.send_header('Content-Length','0');self.end_headers();return
            time.sleep(.22)
            if body['model'] == 'runtime-timeout': time.sleep(2)
            if body.get('tools'):
                payload = json.loads(body['messages'][1]['content'])
                node = next(n['id'] for n in payload['nodes'] if n['selectable'])
                submission = {'batch_id':payload['batch_id'], 'assignments':[{'file_id':f['id'],'node_id':node,'reason':'fixture'} for f in payload['files']]}
                message = {'role':'assistant','content':'','tool_calls':[{'id':uuid.uuid4().hex,'type':'function','function':{'name':'submit_classifications','arguments':json.dumps(submission)}}]}
            else:
                content = body['messages'][-1]['content']
                system = body['messages'][0]['content']
                if '先查看文件类型统计总览' in system:
                    context=json.loads(content);answer=json.dumps({'summary':'fixture overview','inspect_order':[g['id'] for g in context['groups']]})
                elif '按类型检查当前批次文件' in system:
                    answer=json.dumps({'summary':'fixture type summary'})
                elif 'DownloadSweeper 的文件整理助手' in system:
                    answer=json.dumps({'message':'fixture proposal','changes':[]})
                elif content == 'Reply with OK.': answer = 'OK'
                else:
                    data=json.loads(content)
                    if body['model']=='runtime-rename-fail' and data['name'].startswith('000') and Mock.fail_once:
                        Mock.fail_once=False; answer='invalid response'
                    else: answer=json.dumps({'name':'renamed_'+data['name'],'reason':'fixture'})
                message={'role':'assistant','content':answer}
            result={'choices':[{'message':message,'finish_reason':'stop'}],'usage':{'prompt_tokens':100,'completion_tokens':20}}
            data=json.dumps(result).encode()
            self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(data)));self.end_headers()
            try:self.wfile.write(data)
            except (BrokenPipeError,ConnectionResetError,ConnectionAbortedError):pass
        finally:
            with self.lock: Mock.active-=1

def main():
    mock=ThreadingHTTPServer(('127.0.0.1',0),Mock)
    threading.Thread(target=mock.serve_forever,daemon=True).start()
    directory=ROOT/'artifacts'/('runtime-'+uuid.uuid4().hex[:8]);directory.mkdir(parents=True)
    config=directory/'config.toml'
    config.write_text('[llm]\nendpoint='+json.dumps(f'http://127.0.0.1:{mock.server_port}/v1')+'\nmodel="runtime-fixture"\napi_key_env="DS_TEST_EMPTY"\ncontext_length=1000000\nmax_output_tokens=64000\nparallel_requests=3\n',encoding='utf-8')
    env={k:v for k,v in os.environ.items() if 'API_KEY' not in k and not k.startswith('DS_MODEL_KEY_') and k!='DS_TEST_EMPTY'}
    log=(directory/'server.log').open('w',encoding='utf-8')
    process=subprocess.Popen([str(ROOT/'target/debug'/('ds-web.exe' if os.name == 'nt' else 'ds-web')),'--port','0','--config',str(config),'--data-dir',str(directory/'data')],cwd=ROOT,env=env,stdout=log,stderr=log,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
    try:
        base=None
        for _ in range(150):
            for word in (directory/'server.log').read_text(encoding='utf-8').split():
                if word.startswith('http://127.0.0.1:'):base=word.rstrip('/')
            if base:break
            time.sleep(.1)
        assert base,'startup failed'
        def get(path):return json.load(urlopen(base+path,timeout=10))
        token=get('/api/bootstrap')['token']
        def post(action,context=None,**args):
            body={'action':action,**args}
            if context:body.update(task_id=context['id'],revision=context['revision'])
            try:return json.load(urlopen(Request(base+'/api/action',data=json.dumps(body).encode(),headers={'Content-Type':'application/json','x-ds-token':token}),timeout=10))
            except HTTPError as e:raise AssertionError(e.read().decode()) from e
        def wait(job,status='completed'):
            for _ in range(600):
                state=get('/api/bootstrap')
                if not state['job']:
                    assert state['last_job']['id']==job['job']['id']
                    assert state['last_job']['status']==status,state['last_job']
                    return get('/api/tasks/'+job['job']['task_id'])
                time.sleep(.03)
            raise AssertionError('job timeout')
        def configure(**changes):
            cfg=get('/api/bootstrap')['config'];cfg['llm'].update(changes);post('config',config=cfg)
        def planning(folder,mode='organize'):
            task=post('create',root=str(folder),mode=mode);task=wait(post('scan',task))
            task=post('advance',task);task=post('advance',task)
            if mode=='rename':task=post('rename_scope',task,extensions=['txt'],web_search=False)
            task=post('advance',task)
            return wait(task) if 'job' in task else task
        folder=directory/'files';folder.mkdir()
        for i in range(120):(folder/f'{i:03}.txt').write_text('test evidence',encoding='utf-8')
        before={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in folder.iterdir()}
        task=planning(folder)
        Mock.maximum=0;start=time.monotonic();task=wait(post('plan_ai',task,batch_size=20));parallel=time.monotonic()-start
        assert 1<Mock.maximum<=3,Mock.maximum
        assert task['classification']['completed']==120 and not task['pending_calls']
        assert len({c['id'] for c in task['calls']})==len(task['calls'])
        configure(parallel_requests=1)
        start=time.monotonic();task=wait(post('plan_ai',task,batch_size=20));serial=time.monotonic()-start
        assert parallel<serial*.85,(parallel,serial)
        print(f'PASS bounded concurrency: {parallel:.2f}s parallel vs {serial:.2f}s serial')

        # Large context admits payloads beyond the former fixed 24 KiB limit.
        long=directory/'long';long.mkdir()
        for i in range(240):(long/(str(i)+'文件描述'*14+'.txt')).write_text('fixture')
        configure(parallel_requests=3)
        large=wait(post('plan_ai',planning(long),batch_size=512))
        events=get('/api/tasks/'+large['id']+'/trajectory')
        requests=[e['detail'] for e in events if e['kind']=='llm_request']
        assert max(r['input_text_bytes'] for r in requests)>24*1024
        assert len(large['classification']['batches'])==1,large['classification']['batches']
        print('PASS large context packs 240 files into one request beyond 24 KiB')

        # Type groups run concurrently; each group's cumulative summary stays ordered.
        mixed=directory/'mixed';mixed.mkdir()
        for extension in ('txt','mp4','png'):
            for i in range(12):(mixed/f'{i:03}.{extension}').write_text('fixture')
        inspection=post('create',root=str(mixed),mode='organize')
        inspection=wait(post('scan',inspection));inspection=post('advance',inspection);inspection=post('advance',inspection)
        Mock.maximum=0
        inspection=wait(post('suggest_tree',inspection,message='检查并设计目录'))
        assert Mock.maximum==3 and inspection['inspection']['status']=='complete'
        assert sum(g['inspected'] for g in inspection['inspection']['groups'])==36
        print('PASS directory inspection runs type groups concurrently and commits every summary')

        # Completed per-file rename suggestions survive another worker's invalid answer.
        small=directory/'rename';small.mkdir()
        for i in range(6):(small/f'{i:03}.txt').write_text('fixture')
        rename=planning(small,'rename');original=rename['operations']
        configure(model='runtime-rename-fail')
        rename=wait(post('rename',rename),'failed')
        saved=dict(rename['rename_checkpoint']['results'])
        assert saved and len(saved)<6 and rename['operations']==original
        count=len(Mock.calls)
        rename=wait(post('rename',rename))
        assert len(Mock.calls)-count==6-len(saved)
        assert all(rename['rename_checkpoint']['results'][k]==v for k,v in saved.items())
        assert rename['rename_checkpoint']['status']=='complete' and len(rename['operations'])==6
        print('PASS parallel rename checkpoints, resume skips finished files, original plan preserved on error')

        # A finite budget makes requests wait for reservations, without exceeding the cap.
        configure(model='runtime-fixture',max_output_tokens=16000)
        cfg=get('/api/bootstrap')['config'];cfg['token_budget']=20000;post('config',config=cfg)
        budget=planning(folder);budget=wait(post('plan_ai',budget,batch_size=20))
        reserved=0
        for event in get('/api/tasks/'+budget['id']+'/trajectory'):
            if event['kind']=='llm_request':reserved+=event['detail']['reserved_tokens']
            elif event['kind']=='llm_response':
                request=next(e['detail'] for e in get('/api/tasks/'+budget['id']+'/trajectory') if e['kind']=='llm_request' and e['detail']['id']==event['detail']['id'])
                reserved-=request['reserved_tokens'];reserved+=event['detail']['usage']['prompt_tokens']+event['detail']['usage']['completion_tokens']
            assert reserved<=20000,reserved
        assert not budget['pending_calls']
        print('PASS durable reservations never oversubscribe finite task budget')

        configure(model='runtime-timeout',request_timeout_seconds=1)
        uncertain=post('create',root=str(small),mode='organize')
        uncertain=wait(post('test_connection',uncertain),'failed')
        assert len(uncertain['pending_calls'])==1 and not uncertain['calls']
        # The reservation is in task.json, not merely in a live worker's memory.
        stored=json.loads((directory/'data/tasks'/uncertain['id']/'task.json').read_text(encoding='utf-8'))
        assert stored['pending_calls']==uncertain['pending_calls']
        configure(model='runtime-denied')
        rejected=post('create',root=str(small),mode='organize')
        rejected=wait(post('test_connection',rejected),'failed')
        assert not rejected['pending_calls'] and not rejected['calls']
        assert {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in folder.iterdir()}==before
        imported=post('import',task=rename)
        assert imported.get('rename_checkpoint') is None and imported['pending_calls']==[] and not imported['scanned']
        print('PASS unconfirmed usage survives failure, import invalidates checkpoints, no files moved')
    finally:
        process.terminate();process.wait(timeout=10);mock.shutdown();log.close()

if __name__=='__main__':main()
