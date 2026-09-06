"""Owned loopback R4 acceptance: real pause, process death, restart, replay gates and archives."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import json, os, subprocess, sys, threading, time, uuid

ROOT = Path(__file__).resolve().parents[1]
class Mock(BaseHTTPRequestHandler):
    calls = 0
    def log_message(self, *args): pass
    def do_POST(self):
        self.rfile.read(int(self.headers['Content-Length']))
        Mock.calls += 1
        time.sleep(5)
        data = json.dumps({'choices':[{'message':{'role':'assistant','content':'OK'},'finish_reason':'stop'}],
                           'usage':{'prompt_tokens':20,'completion_tokens':2}}).encode()
        try:
            self.send_response(200); self.send_header('Content-Type','application/json')
            self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError): pass

def main():
    directory = ROOT/'artifacts'/('checkpoints-'+uuid.uuid4().hex[:8]); directory.mkdir(parents=True)
    root = directory/'Downloads'; root.mkdir()
    for index in range(6000): (root/f'file-{index:05}.txt').write_text(str(index),encoding='utf-8')
    mock = ThreadingHTTPServer(('127.0.0.1',0),Mock)
    threading.Thread(target=mock.serve_forever,daemon=True).start()
    config = directory/'config.toml'
    config.write_text(f'[llm]\nendpoint="http://127.0.0.1:{mock.server_port}/v1"\nmodel="checkpoint-fixture"\napi_key_env="DS_CHECKPOINT_TEST_MISSING"\n',encoding='utf-8')
    env = {k:v for k,v in os.environ.items() if 'API_KEY' not in k and not k.startswith('DS_MODEL_KEY_')}
    process = None; log = None; base = None; token = None
    def start():
        nonlocal process,log,base,token
        log = (directory/'server.log').open('w',encoding='utf-8')
        process = subprocess.Popen([str(ROOT/'target/debug'/('ds-web.exe' if os.name=='nt' else 'ds-web')),
            '--port','0','--config',str(config),'--data-dir',str(directory/'data')],cwd=ROOT,env=env,stdout=log,stderr=log,
            creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
        base = None
        for _ in range(200):
            urls = [s for s in (directory/'server.log').read_text(encoding='utf-8').split() if s.startswith('http://127.0.0.1:')]
            if urls: base=urls[-1].rstrip('/'); break
            assert process.poll() is None, 'server failed to start'
            time.sleep(.05)
        assert base; token=get('/api/bootstrap')['token']
    def stop():
        if process and process.poll() is None: process.kill(); process.wait(timeout=10)
        if log: log.close()
    def get(path): return json.load(urlopen(base+path,timeout=20))
    def post(action,current=None,**args):
        payload={'action':action,**args}
        if current:payload.update(task_id=current['id'],revision=current['revision'])
        return json.load(urlopen(Request(base+'/api/action',data=json.dumps(payload).encode(),
            headers={'Content-Type':'application/json','x-ds-token':token}),timeout=20))
    def finish(job,status='completed'):
        for _ in range(1600):
            b=get('/api/bootstrap')
            if not b['job']:
                assert b['last_job']['id']==job['id'],b
                assert b['last_job']['status']==status,b['last_job']
                return b['last_job']
            time.sleep(.02)
        raise AssertionError('job timeout')
    def rejected(action,**args):
        try:post(action,**args);raise AssertionError('unsafe resume accepted')
        except HTTPError as e: assert e.code==400; return json.load(e)['error']
    try:
        start(); task=post('create',root=str(root))
        # Pause before a scan can publish its private draft; the old snapshot survives.
        job=post('scan',task)['job']; post('cancel',job_id=job['id']); finish(job,'paused')
        unchanged=get('/api/tasks/'+task['id']); assert unchanged['entries']==[] and not unchanged['scanned']
        assert get('/api/jobs')[0]['resumable']
        # A mutation invalidates the old checkpoint, even when it looks harmless.
        task=post('dismiss_proposal',unchanged)
        assert '已改变' in rejected('resume_job',job_id=job['id'])
        assert not next(j for j in get('/api/jobs') if j['id']==job['id'])['resumable']
        # Kill the actual process during a fresh scan, then continue using the same ID.
        interrupted=post('scan',task)['job']; stop(); start()
        recovered=next(j for j in get('/api/jobs') if j['id']==interrupted['id'])
        assert recovered['status']=='interrupted' and recovered['resumable'],recovered
        continued=post('resume_job',job_id=interrupted['id'])['job']; assert continued['id']==interrupted['id']
        finish(continued); task=get('/api/tasks/'+task['id']); assert len(task['entries'])==6000
        print('PASS durable pause, stale-checkpoint rejection, process death and scan restoration',flush=True)
        # >3s work has continuing heartbeats and cancellation. Unknown billing cannot be retried as resume.
        job=post('test_connection',task)['job']; time.sleep(3.2)
        running=get('/api/bootstrap')['job']; assert running and running['saved_at']!=job['saved_at']
        post('cancel',job_id=job['id']); paused=finish(job,'paused'); assert not paused['resumable']
        task=get('/api/tasks/'+task['id']); assert len(task['pending_calls'])==1
        count=Mock.calls; stop(); start()
        assert not next(j for j in get('/api/jobs') if j['id']==job['id'])['resumable']
        rejected('resume_job',job_id=job['id']); assert Mock.calls==count
        print('PASS >3s heartbeat, pause and restart keep uncertain usage without resending',flush=True)
        # Export/import are visible jobs. Import result ID is stable and remains read-only.
        task=get('/api/tasks/'+task['id'])
        export=post('archive_export',task)['job']; finish(export)
        archive=get('/api/jobs/'+export['id']+'/result'); assert json.loads(archive['payload'])['task']==task
        imported=post('archive_import',**{'task':archive})['job']; finish(imported)
        result=get('/api/jobs/'+imported['id']+'/result'); assert result['archive_id']==imported['id']
        assert get('/api/archives/'+result['archive_id'])['payload']==archive['payload']
        assert len(get('/api/tasks'))==1
        stop(); start(); assert get('/api/jobs/'+export['id']+'/result')['checksum']==archive['checksum']
        print('PASS background archives, durable downloadable results and read-only import',flush=True)
        assert Mock.calls==count
        assert all((root/f'file-{i:05}.txt').read_text(encoding='utf-8')==str(i) for i in range(6000))
        print('PASS all fixture files unchanged; no additional model charges',flush=True)
        if '--serve' in sys.argv:
            print('GUI fixture: '+base,flush=True); print('Task: '+task['id'],flush=True)
            print('Stop file: '+str(directory/'stop'),flush=True)
            while not (directory/'stop').exists(): time.sleep(.2)
    finally: stop(); mock.shutdown()

if __name__=='__main__': main()
