"""Fixed loopback replies for the optional interactive GUI demo; not a test server.

Automated tests use the independent Rust mock in crates/web/tests/support.
"""
from http.server import BaseHTTPRequestHandler
import json
import time


class Mock(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def respond(self, data):
        try:
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        messages = request.get('messages', [])
        system = messages[0]['content']
        content = messages[-1]['content']
        content = content[0]['text'] if isinstance(content, list) else content
        tools = request.get('tools', [])
        if any(t['function']['name'] == 'submit_classifications' for t in tools):
            context = json.loads(messages[1]['content'])
            time.sleep(.35)
            name = 'submit_classifications'
            args = {'batch_id': context['batch_id'], 'assignments': [
                {'file_id': f['id'], 'node_id': next(n['id'] for n in context['nodes'] if n['selectable']),
                 'reason': '本地演示：选择当前分支第一个候选'} for f in context['files']]}
            if request.get('model') == 'demo-evidence':
                results = [json.loads(m['content']) for m in messages if m['role'] == 'tool']
                already = {f['file_id'] for r in results for f in r.get('files', [])}
                eligible = list(dict.fromkeys(f['id'] for f in context['files'] + context['examples']
                                              if f['evidence'] != 'none' and f['id'] not in already))
                if eligible:
                    name, args = 'read_file_evidence', {'file_ids': eligible[:4]}
            message = {'role': 'assistant', 'content': '', 'tool_calls': [
                {'id': 'demo_' + str(len(messages)), 'type': 'function',
                 'function': {'name': name, 'arguments': json.dumps(args, ensure_ascii=False)}}]}
            finish = 'tool_calls'
        else:
            if content == 'Reply with OK.':
                answer = 'OK'
            elif '先查看文件类型统计总览' in system:
                context = json.loads(content)
                answer = {'summary': '本地演示：按文件类型逐组检查。', 'inspect_order': [g['id'] for g in context['groups']]}
            elif '按类型检查当前批次文件' in system:
                answer = {'summary': '本地演示：可按工作和学习用途细分，证据不足时保留原分类。'}
            elif '依据现有信息提出清晰简洁文件名' in system:
                answer = {'name': '可读_' + json.loads(content)['name'], 'reason': '本地演示命名建议'}
            elif '复核清理候选' in system:
                answer = {'suggestions': [{'index': c['index'], 'reason': '请确认用途与备份后决定是否清理。'} for c in json.loads(content)]}
            else:
                context = json.loads(messages[1]['content'].split('：', 1)[1])
                changes = []
                if context.get('editable') and context.get('scene') == 'tree' and context.get('nodes'):
                    node = context['nodes'][0]
                    changes = [{'kind': 'node', 'target': node['id'], 'after': {'note': '保留原有分类，减少不必要的移动。'}}]
                answer = {'message': '本地固定建议，仅用于演示审查与合并操作。', 'changes': changes}
            message = {'role': 'assistant', 'content': answer if isinstance(answer, str) else json.dumps(answer, ensure_ascii=False)}
            finish = 'stop'
        self.respond(json.dumps({'choices': [{'finish_reason': finish, 'message': message}],
                                 'usage': {'prompt_tokens': 321, 'completion_tokens': 45}}).encode())
