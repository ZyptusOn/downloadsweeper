"""Run workflow HTTP suites with an owned server and a local mock; no live API keys."""
from pathlib import Path
from http.server import ThreadingHTTPServer
import os, subprocess, sys, threading, time, uuid
from mock_llm import Mock
ROOT=Path(__file__).resolve().parents[1]
mock=ThreadingHTTPServer(('127.0.0.1',3190),Mock)
threading.Thread(target=mock.serve_forever,daemon=True).start()
directory=ROOT/'artifacts'/('workflow-runtime-'+uuid.uuid4().hex[:8]);directory.mkdir(parents=True)
config=directory/'config.toml'
config.write_text('[llm]\nendpoint="http://127.0.0.1:3190/v1"\nmodel="local-test"\napi_key_env="DS_TEST_EMPTY"\ncontext_length=256000\nmax_output_tokens=32768\nparallel_requests=1\n',encoding='utf-8')
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
    env['DS_TEST_URL']=base
    for name in sys.argv[1:] or ['smoke_test.py','planning_regression.py','classification_regression.py','inspection_regression.py','tree_structure_regression.py','ai_proposal_regression.py']:
        assert Path(name).name==name and name.endswith('.py')
        subprocess.run([sys.executable,str(ROOT/'scripts'/name)],cwd=ROOT,env=env,check=True)
finally:
    process.terminate();process.wait(timeout=10);mock.shutdown();log.close()
