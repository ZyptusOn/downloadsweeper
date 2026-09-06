"""Local OpenAI-compatible fixture for integration tests. No external requests."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import json
import time

class Mock(BaseHTTPRequestHandler):
    truncated_batches = set()

    def log_message(self, *args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        messages = request.get('messages', [])
        raw = json.dumps(messages, ensure_ascii=False)
        Path('artifacts').mkdir(exist_ok=True)
        with open('artifacts/mock-requests.jsonl', 'a', encoding='utf-8') as out:
            out.write(json.dumps({'path': self.path, 'body': request}, ensure_ascii=False) + '\n')
        if self.path == '/search':
            response = json.dumps({'results':[{'title':'Public fixture evidence','url':'https://example.com/fixture','content':'Local search fixture, no network request.'}]}).encode()
            self.respond(response)
            return
        if 'slow-fixture' in raw or request.get('model') == 'planning-pause-fixture':
            time.sleep(8)
        if request.get('tools') and any(t['function']['name']=='submit_classifications' for t in request['tools']):
            self.classify(request)
            return
        if '先查看文件类型统计总览' in messages[0]['content']:
            context = json.loads(messages[-1]['content'])
            answer = {'summary':'本地测试总览：先检查可读数量最多的类型，再依次检查其他类型。', 'inspect_order':[g['id'] for g in sorted(context['groups'], key=lambda g: -g['eligible'])]}
        elif '按类型检查当前批次文件' in messages[0]['content']:
            content = messages[-1]['content']
            context = json.loads(content[0]['text'] if isinstance(content, list) else content)
            if request.get('model') == 'inspection-pause-fixture' and context['batch'] == 2:
                time.sleep(4)
            answer = {'summary':f"本地测试：{context['type_name']}已检查 {context['already_inspected'] + len(context['files'])} 个文件，可按工作用途和学习用途进一步区分；缺少内容依据时保持原分类。"}
        elif messages[-1]['content'] == 'Reply with OK.':
            answer = 'OK'
        elif '将文件分到给定候选类别' in messages[0]['content']:
            content = messages[-1]['content']
            context = json.loads(content[0]['text'] if isinstance(content, list) else content)
            answer = {'node_id': context['categories'][0]['id'], 'reason': '本地测试模型：选择当前分支的第一个候选'}
        elif '依据现有信息提出清晰简洁文件名' in messages[0]['content']:
            content = messages[-1]['content']
            context = json.loads(content[0]['text'] if isinstance(content, list) else content)
            name = context['name']
            answer = {'name': '可读_' + name, 'reason': '本地测试模型命名建议'}
        elif '复核清理候选' in messages[0]['content'] or '对大文件和临时小文件' in messages[0]['content']:
            context = json.loads(messages[-1]['content'])
            answer = {'suggestions':[{'index':c['index'],'reason':'本地测试模型：请确认用途与备份后再自行决定是否清理。'} for c in context]}
        else:
            context = json.loads(messages[1]['content'].split('：', 1)[1])
            changes = []
            if context['scene'] == 'review' and context.get('editable'):
                model=request.get('model')
                if model == 'review-split-fixture':
                    changes=[{'kind':'node','target':'review-video','after':{'name':'视频','parent':'root','rule_type':'simple','extensions':['mp4','mkv']}}]
                elif model == 'review-placement-fixture':
                    entry=next(f for f in context['review_files'] if f.get('name')=='notes.txt')
                    changes=[{'kind':'node','target':'review-notes','after':{'name':'笔记','parent':'docs','rule_type':'complex','note':'用户指定的笔记'}},
                             {'kind':'placement','target':entry['id'],'after':{'node_id':'review-notes'}}]
                elif model == 'review-keep-fixture':
                    entry=next(f for f in context['review_files'] if f.get('name')=='clip.mp4')
                    changes=[{'kind':'placement','target':entry['id'],'after':{'node_id':None}}]
                elif model == 'review-invalid-fixture':
                    changes=[{'kind':'placement','target':'f99999','after':{'node_id':'docs'}}]
                elif model in ['review-semantic-fixture', 'review-semantic-resume-fixture']:
                    changes=[{'kind':'node','target':'review-semantic','after':{'name':'学习笔记','parent':'docs','rule_type':'complex','note':'学习文本和课程笔记'}}]
            elif context['scene'] == 'tree' and context.get('editable') and context.get('nodes'):
                if request.get('model') == 'duplicate-template-fixture':
                    photo=next(n for n in context['nodes'] if n['name']=='照片')
                    changes=[
                        {'kind':'node','target':'duplicate-photo','after':{'name':'照片','parent':photo['parent'],'rule_type':'complex','note':'实拍照片，排除截图和设计素材'}},
                        {'kind':'node','target':'photo-travel','after':{'name':'旅行照片','parent':'duplicate-photo','rule_type':'complex','note':'旅行实拍照片'}}]
                elif request.get('model') == 'new-top-level-fixture':
                    changes = [
                        {'kind':'node','target':'new-music','after':{'name':'音频','parent':'root','rule_type':'simple','extensions':['mp3','wav','flac'],'note':'音乐与录音'}},
                        {'kind':'node','target':'new-recordings','after':{'name':'录音','parent':'new-music','rule_type':'complex','note':'会议与课堂录音'}},
                        {'kind':'node','target':'new-meetings','after':{'name':'会议','parent':'new-recordings','rule_type':'complex','note':'会议录音'}}]
                elif request.get('model') in ['tree-structure-fixture', 'invalid-tree-fixture']:
                    video = next(n for n in context['nodes'] if n['parent'] == 'root' and 'mp4' in n['extensions'])
                    docs = next(n for n in context['nodes'] if n['parent'] == 'root' and 'pdf' in n['extensions'])
                    children = [n for n in context['nodes'] if n['parent'] == video['id']]
                    changes = [
                        {'kind':'node','target':'fixture-campus','after':{'name':'校园记录/学业','parent':'fixture-topics','rule_type':'complex','note':'校园纪实与集体活动；游戏片段交给游戏素材，无法确认的保留在父目录。'}},
                        {'kind':'node','target':'fixture-topics','after':{'name':'专题素材','parent':video['id'],'rule_type':'complex','note':'按用途组织短视频；完整电影保留在影视目录。'}},
                        {'kind':'node','target':children[0]['id'],'after':{'name':'影视长片','note':'完整电影和剧场版；剪辑片段交给专题素材。'}},
                        {'kind':'node','target':children[1]['id'],'after':None},
                        {'kind':'node','target':children[2]['id'],'after':{'parent':'fixture-topics','note':'视频剪辑可复用的素材片段。'}},
                        {'kind':'node','target':docs['id'],'after':{'extensions':docs['extensions']+['htm','xml'],'note':'文档与结构化参考资料。'}},
                    ]
                    if request.get('model') == 'invalid-tree-fixture':
                        changes[0]['after']['parent'] = 'nonexistent-parent'
                else:
                    node = dict(context['nodes'][0])
                    node.pop('example_context', None)
                    node['note'] = '保留原有分类，减少不必要的移动。'
                    changes = [{'kind': 'node', 'target': node['id'], 'after': node}]
            answer = {'message': '这是本地测试模型的建议。建议为当前目录补充分类备注，供你审查后合并。', 'changes': changes}
        text = answer if isinstance(answer, str) else json.dumps(answer, ensure_ascii=False)
        response = json.dumps({'choices': [{'finish_reason': 'stop', 'message': {'role': 'assistant', 'content': text}}],
                               'usage': {'prompt_tokens': 321, 'completion_tokens': 45}}).encode()
        if request.get('model') in ['truncated-reasoning-fixture', 'truncated-json-fixture', 'empty-answer-fixture', 'invalid-json-fixture']:
            model = request['model']
            content = '{"message":"unfinished' if model == 'truncated-json-fixture' else 'not valid JSON' if model == 'invalid-json-fixture' else ''
            response = json.dumps({'choices': [{'finish_reason': 'length' if model.startswith('truncated-') else 'stop', 'message': {'role': 'assistant', 'content': content}}], 'usage': {'prompt_tokens': 321, 'completion_tokens': request['max_tokens'] if model.startswith('truncated-') else 45}}).encode()
        if request.get('model') == 'missing-usage-fixture':
            data = json.loads(response)
            del data['usage']
            response = json.dumps(data).encode()
        self.respond(response)

    def classify(self, request):
        messages=request['messages']
        context=json.loads(messages[1]['content'])
        model=request.get('model','')
        results=[json.loads(m['content']) for m in messages if m['role']=='tool']
        if model=='classification-pause-fixture' and context['batch_id']=='b2':
            time.sleep(5)
        name='submit_classifications'
        assignments=[{'file_id':f['id'],'node_id':next(n['id'] for n in context['nodes'] if n['selectable']),
                      'reason':'本地批量分类测试'} for f in context['files']]
        if model=='directory-classification-fixture':
            for assignment in assignments:
                assignment['node_id']=next(n['id'] for n in context['nodes'] if n['selectable'] and '文档' in n['name'])
        if model in ['review-semantic-fixture', 'review-semantic-resume-fixture']:
            time.sleep(.35)
            for assignment in assignments:
                assignment['node_id']=next(n['id'] for n in context['nodes'] if n['selectable'] and '学习笔记' in n['name'])
        args={'batch_id':context['batch_id'],'assignments':assignments}
        if model in ['classification-evidence-fixture','deepseek-evidence-fixture','directory-classification-fixture']:
            already={f['file_id'] for result in results for f in result.get('files',[])}
            eligible=[f['id'] for f in context['files']+context['examples'] if f['id'] not in already and f['evidence']!='none']
            if eligible:
                name='read_file_evidence'
                args={'file_ids':list(dict.fromkeys(eligible))[:4]}
            if model.startswith('deepseek') and any(m['role']=='assistant' and m.get('reasoning_content')!='fixture continuation' for m in messages):
                self.send_error(400,'Missing reasoning continuation')
                return
        elif model=='classification-forbidden-read-fixture' and not results:
            name='read_file_evidence'
            args={'file_ids':['../../private.xlsx']}
        elif model=='classification-permission-fixture' and not results:
            name='read_file_evidence'
            args={'file_ids':[f['id'] for f in context['files']][:4]}
        elif model=='classification-invalid-node-fixture':
            assignments[0]['node_id']='n999999'
        elif model=='classification-incomplete-fixture':
            assignments.pop()
        elif model=='classification-duplicate-fixture':
            assignments[-1]=assignments[0]
        elif model=='classification-path-fixture':
            assignments[0]['destination']='../../outside.txt'
        elif model=='classification-null-fixture':
            for assignment in assignments: assignment['node_id']=None
        elif model=='classification-missing-node-fixture':
            del assignments[0]['node_id']
        if model=='parallel-progress-fixture': time.sleep(0.7 if context['batch_id']=='b1' else 0.35)
        if model=='invalid-json-fixture':
            message={'role':'assistant','content':'not valid JSON'}
            finish='stop'
        else:
            message={'role':'assistant','content':'','tool_calls':[{'id':'call_fixture_'+str(len(messages)),
                     'type':'function','function':{'name':name,'arguments':json.dumps(args,ensure_ascii=False)}}]}
            if model.startswith('deepseek'): message['reasoning_content']='fixture continuation'
            finish='tool_calls'
        response={'choices':[{'finish_reason':finish,'message':message}],
                  'usage':{'prompt_tokens':321,'completion_tokens':45}}
        truncate = model == 'classification-truncated-fixture'
        if model in ['classification-truncated-once-fixture', 'review-semantic-resume-fixture'] and context['batch_id']=='b2':
            key = messages[1]['content']
            truncate = key not in self.truncated_batches
            self.truncated_batches.add(key)
        if truncate:
            # Valid HTTP/response JSON, but an incomplete tool argument string.
            message['tool_calls'][0]['function']['arguments']='{"batch_id":"'+context['batch_id']+'","assignments":['
            response['choices'][0]['finish_reason']='length'
            response['usage']['completion_tokens']=request['max_tokens']
        if model=='missing-usage-fixture': del response['usage']
        self.respond(json.dumps(response).encode())

    def respond(self, response):
        try:
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(response)))
            self.end_headers()
            self.wfile.write(response)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass

if __name__ == '__main__':
    print('Mock LLM fixture at http://127.0.0.1:3190/v1', flush=True)
    ThreadingHTTPServer(('127.0.0.1', 3190), Mock).serve_forever()
