"""Exercise the CLI adapter using its own config, task store and dummy files."""
from pathlib import Path
import json, os
import subprocess
import uuid

project = Path(__file__).resolve().parents[1]
binary = project / 'target/debug' / ('ds.exe' if __import__('os').name == 'nt' else 'ds')
base = project / 'artifacts' / ('cli-fixture-' + uuid.uuid4().hex[:8])
root = base / 'Downloads'
root.mkdir(parents=True)
(root / 'sample.txt').write_text('CLI roundtrip fixture',encoding='utf-8')
(base / 'config.toml').write_text('scan_root = '+json.dumps(str(root))+'\n',encoding='utf-8')

def run(*args, success=True):
    result = subprocess.run([str(binary),*args],cwd=base,env={**os.environ, 'DS_DATA_DIR':str(base/'.ds-data'), 'DS_CONFIG':str(base/'config.toml')},capture_output=True,text=True,encoding='utf-8',timeout=15)
    assert (result.returncode == 0) == success, result.stderr
    return result.stdout

task_id = run('create',str(root)).strip()
run('execute',task_id,success=False)
for _ in range(3):
    run('next',task_id)
task = json.loads(run('show',task_id))
assert task['phase']==3 and task['plan_source']=='rules' and not task['calls']
assert len(task['operations'])==1 and (root/'sample.txt').exists()
run('next',task_id)
task = json.loads(run('show',task_id))
assert task['phase']==4 and len(task['operations'])==1
run('approve',task_id)
run('execute',task_id)
assert not (root/'sample.txt').exists()
run('rollback',task_id)
assert (root/'sample.txt').read_text(encoding='utf-8')=='CLI roundtrip fixture'
run('export',task_id,str(base/'session.json'))
run('import',str(base/'session.json'))
archives=json.loads(run('archives'))
assert len(archives)==1
archive_id=archives[0]['id']
archived=json.loads(run('archive-show',archive_id))
contents=json.loads(archived['payload'])
assert contents['task']['id']==task_id and contents['trajectory']
run('execute',archive_id,success=False)
imported=run('resume',archive_id).strip()
assert imported != task_id
assert not json.loads(run('show',imported))['scanned']
print('PASS: CLI staged gating, shared workflow, plan/approve/execute/restore, JSON export/import')
