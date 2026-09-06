"""Exercise native batch classification tools using only fixture files and a local mock."""
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError
import copy
import hashlib
import json
import os
import time
import uuid
import struct
import zlib

BASE=os.environ.get('DS_TEST_URL','http://127.0.0.1:3189')
def get(path): return json.load(urlopen(BASE+path,timeout=10))
initial=get('/api/bootstrap')
assert not initial['job']
assert initial['config']['llm']['endpoint']=='http://127.0.0.1:3190/v1', 'Use isolated local mock config'
token=initial['token']
def post(action,context=None,**args):
    data={'action':action,**args}
    if context: data.update(task_id=context['id'],revision=context['revision'])
    return json.load(urlopen(Request(BASE+'/api/action',data=json.dumps(data).encode(),headers={'Content-Type':'application/json','x-ds-token':token}),timeout=10))
def wait(result,status='completed'):
    deadline=time.monotonic()+30
    while time.monotonic()<deadline:
        state=get('/api/bootstrap')
        if not state['job']:
            assert state['last_job']['id']==result['job']['id'] and state['last_job']['status']==status,state['last_job']
            return get('/api/tasks/'+result['job']['task_id'])
        time.sleep(.04)
    raise AssertionError('Job timed out')
def events(task): return get('/api/tasks/'+task['id']+'/trajectory')

wire_log=Path(__file__).resolve().parents[1]/'artifacts/mock-requests.jsonl'
def wire_since(offset):
    with wire_log.open('rb') as stream:
        stream.seek(offset)
        return [json.loads(line)['body'] for line in stream if line.strip()]

def planning(root,permissions=None):
    task=wait(post('scan',post('create',root=str(root),mode='organize')))
    task=post('advance',task)
    if permissions: task=post('permissions',task,permissions=permissions)
    task=post('advance',task)
    return wait(post('advance',task))

root=Path(__file__).resolve().parents[1]/'artifacts'/('classification-fixture-'+uuid.uuid4().hex[:8])
root.mkdir()
for i in range(130): (root/f'clip_{i:03}.mp4').write_text('dummy video',encoding='utf-8')
(root/'private.xlsx').write_text('DO_NOT_SEND_PRIVATE_TABLE',encoding='utf-8')
(root/'Portable').mkdir()
(root/'Portable/app.exe').write_text('DO_NOT_READ_ATOMIC_DIRECTORY',encoding='utf-8')
def hashes(path): return {str(p.relative_to(path)):hashlib.sha256(p.read_bytes()).hexdigest() for p in path.rglob('*') if p.is_file()}
before=hashes(root)
cfg=copy.deepcopy(initial['config'])
cfg['token_budget']=100000
cfg['llm']['parallel_requests']=1
cfg['llm']['model']='local-test'
cfg['llm']['thinking_mode']=True  # Classification must independently default to standard mode.
cfg['llm']['context_length']=256000
cfg['llm']['max_output_tokens']=32768
report={'checks':[]}
try:
    post('config',config=cfg)
    task=planning(root,{'default':'filename_only','content_slice_bytes':64,'rules':[{'extensions':['xlsx','@folder'],'tier':'none'}]})
    base_plan=copy.deepcopy(task['operations'])
    tree=copy.deepcopy(task['nodes'])
    wire_offset=wire_log.stat().st_size if wire_log.exists() else 0
    task=wait(post('plan_ai',task,batch_size=64))
    run=task['classification']
    assert run['status']=='complete' and run['completed']==130
    assert [b['files'] for b in run['batches']]==[64,64,2],run
    assert len(task['calls'])==3 and all(c['finish_reason']=='tool_calls' for c in task['calls'])
    assert task['nodes']==tree and task['phase']==3
    assert {o['source']:o['id'] for o in task['operations']}=={o['source']:o['id'] for o in base_plan}
    requests=[e['detail'] for e in events(task) if e['kind']=='llm_request']
    assert all(not r['thinking'] and r['input_text_bytes']<=24*1024 for r in requests)
    assert all(set(r['tools'])=={'read_file_evidence','submit_classifications'} for r in requests)
    assert all(r['max_output_tokens']==32768 and r['configured_max_output_tokens']==32768 and not r['output_limit_reasons'] for r in requests)
    wire=wire_since(wire_offset)
    assert len(wire)==3 and all(r['max_tokens']==32768 for r in wire)
    assert not any(o['source'].startswith('Portable/') for o in task['operations'])
    report.update(task_id=task['id'],files=130,api_calls=3,batch_files=[64,64,2],request_text_bytes=[r['input_text_bytes'] for r in requests])
    report['checks']+=['batch_calls_reduce_130_to_3','tree_and_operation_ids_preserved','standard_mode_by_default','bounded_input','atomic_directory_protected']
    report['checks'].append('configured_output_limit_reaches_wire_in_standard_mode')

    # Stop after batch 1, retain the original plan, then resume without repeating batch 1.
    cfg['llm']['model']='classification-pause-fixture'; post('config',config=cfg)
    original=copy.deepcopy(task['operations'])
    pending=post('plan_ai',task,batch_size=64)
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        current=get('/api/tasks/'+task['id'])
        active=get('/api/bootstrap')['job']
        if current.get('classification',{}).get('completed')==64 and active and any(b['id']=='b2' and b['status']=='running' for b in (active.get('parallel') or {}).get('batches',[])): break
        time.sleep(.03)
    else: raise AssertionError('Did not observe first checkpoint')
    post('cancel',job_id=pending['job']['id'])
    task=wait(pending,'paused')
    assert task['classification']['status']=='paused' and task['classification']['completed']==64
    assert task['operations']==original
    calls=len(task['calls'])
    task=wait(post('plan_ai',task,batch_size=64))
    assert task['classification']['status']=='complete' and len(task['calls'])==calls+2
    report['checks'].append('cancel_and_resume_skip_completed_batch')

    # A truncated tool call is billed, never applied, and does not erase batch 1.
    # Unique names let the mock truncate b2 exactly once per test run.
    truncated_root=root/'Truncation';truncated_root.mkdir()
    for i in range(122): (truncated_root/f'{root.name}_{i:03}.mp4').write_text('fixture',encoding='utf-8')
    cfg['llm']['model']='classification-truncated-once-fixture';post('config',config=cfg)
    truncated_task=planning(truncated_root)
    original=copy.deepcopy(truncated_task['operations'])
    truncated_task=wait(post('plan_ai',truncated_task,batch_size=61),'failed')
    run=truncated_task['classification']
    assert [b['files'] for b in run['batches']]==[61,61]
    assert run['completed']==61 and [b['status'] for b in run['batches']]==['complete','pending']
    assert truncated_task['operations']==original and len(truncated_task['calls'])==2
    assert truncated_task['calls'][-1]['usage']=={'prompt_tokens':321,'completion_tokens':32768,'cached_input_tokens':0,'cache_write_tokens':0,'cache_write_1h_tokens':0,'cache_details_known':False}
    assert '请求输出上限 32768' in run['error'] and '设置值 32768' in run['error']
    assert '关闭' not in run['error']  # This call did not use thinking.
    checkpoint=copy.deepcopy(run['batches'][0])
    truncated_task=wait(post('plan_ai',truncated_task,batch_size=61))
    assert truncated_task['classification']['status']=='complete' and len(truncated_task['calls'])==3
    assert truncated_task['classification']['batches'][0]==checkpoint
    report['checks']+=['truncated_tool_response_usage_saved','truncation_preserves_plan_and_completed_batch','retry_skips_completed_batch_after_truncation']

    # Long filenames force smaller batches, even when the numerical cap is 128.
    long_root=root/'Long names';long_root.mkdir()
    for i in range(90): (long_root/(str(i)+'很长的文件描述'*9+'.mp4')).write_text('fixture',encoding='utf-8')
    cfg['llm']['model']='local-test';cfg['llm']['context_length']=32000;post('config',config=cfg)
    long_task=wait(post('plan_ai',planning(long_root),batch_size=128))
    sizes=[b['files'] for b in long_task['classification']['batches']]
    assert sum(sizes)==90 and len(sizes)>1 and max(sizes)<90,sizes
    assert all(e['detail']['input_text_bytes']<=24*1024 for e in events(long_task) if e['kind']=='llm_request')
    report['long_name_batch_files']=sizes
    report['checks'].append('adaptive_batching_by_bytes')
    cfg['llm']['context_length']=256000

    # Small evidence fixture: only the controlled tool may release permitted excerpts.
    evidence_root=root/'Evidence'; evidence_root.mkdir()
    (evidence_root/'notes.txt').write_text('PERMITTED_EVIDENCE_'+'x'*4000,encoding='utf-8')
    (evidence_root/'name_only.txt').write_text('NAME_ONLY_CONTENT_MUST_NOT_LEAK',encoding='utf-8')
    (evidence_root/'secret.xlsx').write_text('DENIED_FILENAME_AND_CONTENT',encoding='utf-8')
    permissions={'default':'filename_only','content_slice_bytes':64,'rules':[
        {'extensions':['xlsx'],'tier':'none'},
        {'extensions':['txt'],'min_bytes':1000,'tier':'content_slice'}]}
    evidence_task=planning(evidence_root,permissions)
    cfg['llm']['model']='deepseek-evidence-fixture';post('config',config=cfg)
    wire_offset=wire_log.stat().st_size
    evidence_task=wait(post('plan_ai',evidence_task,thinking=True,batch_size=64))
    ev=events(evidence_task)
    results=[e['detail']['result'] for e in ev if e['kind']=='classification_tool_result' and e['detail']['tool']=='read_file_evidence']
    assert len(results)==1 and results[0]['files'][0]['bytes']==64
    assert results[0]['files'][0]['truncated']
    assert len(evidence_task['calls'])==2
    wire=wire_since(wire_offset)
    assert len(wire)==2 and all(r['max_tokens']==32768 and r['thinking']['type']=='enabled' for r in wire)
    assert not any('NAME_ONLY_CONTENT_MUST_NOT_LEAK' in json.dumps(r) for r in results)
    report['checks']+=['evidence_only_via_tool','permission_and_slice_limit','thinking_tool_continuation']
    report['checks'].append('configured_output_limit_reaches_wire_in_thinking_mode')

    # Only real context/budget constraints may lower the ceiling, with actionable errors.
    cfg['llm']['model']='classification-truncated-fixture'
    cfg['llm']['context_length']=32000
    cfg['token_budget']=None
    post('config',config=cfg)
    limit_task=planning(evidence_root,permissions)
    original=copy.deepcopy(limit_task['operations'])
    limit_task=wait(post('plan_ai',limit_task),'failed')
    request=[e['detail'] for e in events(limit_task) if e['kind']=='llm_request'][-1]
    assert request['max_output_tokens']==32000-request['estimated_input_tokens']<32768
    assert request['output_limit_reasons']==['上下文剩余空间']
    assert '上下文剩余空间' in limit_task['classification']['error']
    assert limit_task['operations']==original
    cfg['llm']['context_length']=256000
    used=sum(c['usage']['prompt_tokens']+c['usage']['completion_tokens'] for c in limit_task['calls'])
    cfg['token_budget']=used+request['estimated_input_tokens']+2000
    post('config',config=cfg)
    limit_task=wait(post('plan_ai',limit_task),'failed')
    request=[e['detail'] for e in events(limit_task) if e['kind']=='llm_request'][-1]
    assert request['max_output_tokens']==2000
    assert request['output_limit_reasons']==['剩余任务 token 预算（含并行预留）']
    assert '剩余任务 token 预算' in limit_task['classification']['error']
    assert '设置值 32768' in limit_task['classification']['error']
    assert limit_task['operations']==original
    report['checks']+=['context_output_clamp_explained','remaining_budget_output_clamp_explained']
    cfg['token_budget']=100000

    # Both out-of-scope IDs and requests exceeding filename-only permissions are refused.
    for model in ['classification-forbidden-read-fixture','classification-permission-fixture']:
        cfg['llm']['model']=model;post('config',config=cfg)
        evidence_task=wait(post('plan_ai',evidence_task,batch_size=64))
        ev=events(evidence_task)
        latest=[e['detail']['result'] for e in ev if e['kind']=='classification_tool_result' and e['detail']['tool']=='read_file_evidence'][-1]
        if 'forbidden' in model: assert 'error' in latest
        else: assert any(v['status']=='unavailable' for v in latest['files'])
    report['checks'].append('unauthorized_evidence_refused')

    original=copy.deepcopy(evidence_task['operations'])
    # Invalid outputs must never become plans, even if the provider ignores our schema.
    for model in ['classification-invalid-node-fixture','classification-incomplete-fixture','classification-duplicate-fixture','classification-path-fixture','classification-missing-node-fixture']:
        cfg['llm']['model']=model;post('config',config=cfg)
        evidence_task=wait(post('plan_ai',evidence_task,batch_size=64),'failed')
        assert evidence_task['operations']==original
        assert evidence_task['classification']['completed']==0
    report['checks']+=['unknown_node_rejected','missing_file_rejected','duplicate_file_rejected','model_paths_rejected','missing_node_field_rejected']

    # Explicit uncertainty preserves the baseline target; importing cannot trust cached decisions.
    cfg['llm']['model']='classification-null-fixture';post('config',config=cfg)
    evidence_task=wait(post('plan_ai',evidence_task,batch_size=64))
    assert evidence_task['operations']==original
    imported=post('import',task=evidence_task)
    assert imported['classification'] is None and not imported['scanned']
    report['checks']+=['null_preserves_original_target','import_discards_classification']

    # Real tiny PNGs exercise the tool-result/image-message protocol without an external model.
    image_root=root/'Images'; image_root.mkdir()
    def chunk(kind,data): return struct.pack('!I',len(data))+kind+data+struct.pack('!I',zlib.crc32(kind+data)&0xffffffff)
    png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('!IIBBBBB',1,1,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(b'\x00\xff\x00\x00\xff'))+chunk(b'IEND',b'')
    for i in range(8): (image_root/f'picture_{i}.png').write_bytes(png)
    cfg['llm']['model']='classification-evidence-fixture'
    cfg['llm']['multimodal']=True
    post('config',config=cfg)
    image_task=planning(image_root,{'default':'image','content_slice_bytes':64,'rules':[]})
    image_task=wait(post('plan_ai',image_task,batch_size=128))
    assert [b['files'] for b in image_task['classification']['batches']]==[4,4]
    image_calls=[e['detail'] for e in events(image_task) if e['kind']=='llm_request']
    assert [r['image_count'] for r in image_calls]==[0,4,0,4],image_calls
    report['checks'].append('multimodal_evidence_is_bounded_and_tool_driven')
    assert before=={k:v for k,v in hashes(root).items() if k in before}
    assert (root/'clip_000.mp4').exists() and (root/'Portable/app.exe').exists()
    report['checks'].append('no_actual_file_movement')
    (root.parent/'classification-regression.json').write_text(json.dumps(report,indent=2,ensure_ascii=False),encoding='utf-8')
    print(json.dumps(report,ensure_ascii=False))
finally:
    post('config',config=initial['config'])
