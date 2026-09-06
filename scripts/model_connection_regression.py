"""Real HTTP tests of discovery and native protocols; synthetic keys, loopback only."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import threading
import time
from urllib.error import HTTPError
from urllib.parse import urlsplit, parse_qs
from urllib.request import Request, urlopen
import uuid

ROOT = Path(__file__).resolve().parents[1]
KEY = 'synthetic-' + 'connection-test-credential'
requests = []


class Mock(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, value, status=200, headers=None):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        requests.append((self.path, dict(self.headers), None))
        path = urlsplit(self.path).path
        if path.startswith('/denied/'):
            return self.reply({'error': KEY}, 401)
        if path.startswith('/redirect/'):
            return self.reply({}, 302, {'Location': '/leak/models'})
        if path.startswith('/echo/'):
            return self.reply({'data': [{'id': KEY}]})
        if path.startswith('/missing/'):
            return self.reply({}, 404)
        if path.startswith('/native/'):
            cursor = parse_qs(urlsplit(self.path).query).get('after_id')
            if not cursor:
                return self.reply({'data': [{'id': 'claude-opus-4-8', 'max_input_tokens': 1000000,
                                            'max_tokens': 128000, 'capabilities': {'image_input': {'supported': True}}}],
                                   'has_more': True, 'last_id': 'claude-opus-4-8'})
            return self.reply({'data': [{'id': 'claude-sonnet-4-6'}], 'has_more': False})
        return self.reply({'data': [
            {'id': 'deepseek-v4-flash', 'context_length': 96000, 'max_output_tokens': 8000},
            {'id': 'deepseek-v4-flash-vision-exp'}, {'id': 'glm-5.2'}, {'id': 'mimo-v2.5'},
            {'id': 'LongCat-2.0'}, {'id': 'hy3'}, {'id': 'gpt-6-astra'},
            {'id': 'future-unverified'}, {'id': 'text-embedding-3-large'}]})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append((self.path, dict(self.headers), body))
        if self.path.endswith('/messages'):
            return self.reply({'type': 'message', 'content': [{'type': 'text', 'text': 'OK'}],
                               'usage': {'input_tokens': 10, 'output_tokens': 2, 'cache_read_input_tokens': 3}, 'stop_reason': 'end_turn'})
        if self.path.endswith('/responses'):
            return self.reply({'status': 'completed', 'output': [{'type': 'message', 'role': 'assistant',
                                                                 'content': [{'type': 'output_text', 'text': 'OK'}]}],
                               'usage': {'input_tokens': 10, 'output_tokens': 2}})
        return self.reply({'choices': [{'message': {'role': 'assistant', 'content': 'OK'}, 'finish_reason': 'stop'}],
                           'usage': {'prompt_tokens': 10, 'completion_tokens': 2}})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serve', action='store_true', help='Keep an isolated UI fixture running after tests')
    args = parser.parse_args()
    mock = ThreadingHTTPServer(('127.0.0.1', 0), Mock)
    threading.Thread(target=mock.serve_forever, daemon=True).start()
    endpoint = f'http://127.0.0.1:{mock.server_port}/v1'
    directory = ROOT / 'artifacts' / ('model-connections-' + uuid.uuid4().hex[:8])
    directory.mkdir(parents=True)
    config_path = directory / 'config.toml'
    config_path.write_text('[llm]\nendpoint = ' + json.dumps(endpoint) + '\nmodel = "deepseek-v4-flash"\n', encoding='utf-8')
    env = {name: value for name, value in os.environ.items()
           if 'API_KEY' not in name and not name.startswith('DS_MODEL_KEY_')}
    log = (directory / 'server.log').open('w', encoding='utf-8')
    process = subprocess.Popen([str(ROOT / 'target/debug' / ('ds-web.exe' if os.name == 'nt' else 'ds-web')), '--port', '0', '--config', str(config_path),
                                '--data-dir', str(directory / 'data')], cwd=ROOT, env=env,
                               stdout=log, stderr=log, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    try:
        base = None
        for _ in range(150):
            content = (directory / 'server.log').read_text(encoding='utf-8')
            for word in content.split():
                if word.startswith('http://127.0.0.1:'):
                    base = word.rstrip('/')
            if base:
                break
            if process.poll() is not None:
                raise AssertionError('Fixture server exited before startup')
            time.sleep(.1)
        assert base, 'Fixture server startup timed out'

        def get(path):
            return json.load(urlopen(base + path, timeout=5))

        token = get('/api/bootstrap')['token']

        def post(path, payload, expected=200, session=True):
            headers = {'Content-Type': 'application/json'}
            if session:
                headers['x-ds-token'] = token
            request = Request(base + path, data=json.dumps(payload).encode(), headers=headers)
            try:
                response = urlopen(request, timeout=25)
            except HTTPError as error:
                response = error
            result = json.load(response)
            assert response.status == expected, ('unexpected status', response.status)
            assert KEY not in json.dumps(result), 'Credential echoed in response'
            return result

        def discover(url=endpoint, format='auto', saved=False, key=KEY, **kwargs):
            return post('/api/models', dict(endpoint=url, api_format=format, api_key=key, use_saved_key=saved), **kwargs)

        post('/api/models', dict(endpoint=endpoint), expected=403, session=False)
        before = config_path.read_bytes()
        found = discover()
        assert found['status'] == 'ok'
        models = {model['id']: model for model in found['models']}
        assert models['deepseek-v4-flash']['context_length'] == 96000
        assert models['deepseek-v4-flash']['context_source'] == 'api'
        assert models['glm-5.2']['context_source'] == 'preset'
        assert models['deepseek-v4-flash-vision-exp']['vision']
        assert models['future-unverified']['context_length'] is None
        assert not models['text-embedding-3-large']['task_compatible']
        assert config_path.read_bytes() == before and not (directory / '.env').exists()
        assert requests[-1][1].get('authorization') == 'Bearer ' + KEY
        for mode in ('denied', 'missing', 'redirect', 'echo'):
            result = discover(endpoint.replace('/v1', '/' + mode))
            assert result['status'] == 'unavailable' and result['models'] == []
        assert not any(path.startswith('/leak/') for path, _, _ in requests)

        native = discover(endpoint.replace('/v1', '/native/v1'), 'anthropic')
        assert len(native['models']) == 2
        assert requests[-1][1].get('x-api-key') == KEY
        assert requests[-1][1].get('anthropic-version') == '2023-06-01'
        assert 'after_id=' in requests[-1][0]
        print('PASS discovery metadata, unknowns, pagination, auth, errors, redirect isolation and no writes')

        config = get('/api/bootstrap')['config']
        config['llm']['api_key'] = KEY
        saved = post('/api/action', dict(action='config', config=config))
        assert KEY not in config_path.read_text(encoding='utf-8')
        assert (directory / '.env').exists()
        assert saved['has_api_key'] and 'api_key' not in saved['llm']
        assert discover(key='', saved=True)['status'] == 'ok'
        count = len(requests)
        discover(endpoint.replace('/v1', '/different/v1'), key='', saved=True, expected=400)
        assert len(requests) == count
        print('PASS private .env persistence, redacted settings and cross-endpoint saved-key rejection')

        task = post('/api/action', dict(action='create', root=str(directory), mode='organize'))
        cases = [('chat_completions', 'deepseek-v4-flash'), ('chat_completions', 'glm-5.2'),
                 ('chat_completions', 'mimo-v2.5'), ('chat_completions', 'LongCat-2.0'),
                 ('chat_completions', 'hy3'), ('responses', 'gpt-6-astra'), ('anthropic', 'claude-opus-4-8')]
        for format, model in cases:
            config = get('/api/bootstrap')['config']
            config['llm'].update(model=model, api_format=format, thinking_mode=True)
            post('/api/action', dict(action='config', config=config))
            post('/api/action', dict(action='test_connection', task_id=task['id'], revision=task['revision']))
            for _ in range(100):
                state = get('/api/bootstrap')
                if not state['job']:
                    assert state['last_job']['status'] == 'completed', state['last_job']['error']
                    break
                time.sleep(.05)
            else:
                raise AssertionError('Connection test timed out')
            task = get('/api/tasks/' + task['id'])
            path, headers, body = requests[-1]
            assert body['model'] == model
            if format == 'responses':
                assert path.endswith('/responses') and body['store'] is False
                assert 'temperature' not in body and 'max_output_tokens' in body
            elif format == 'anthropic':
                assert path.endswith('/messages') and headers.get('x-api-key') == KEY
                assert body['thinking']['type'] == 'adaptive'
            else:
                assert body['thinking']['type'] == 'enabled'
                if model.startswith('mimo'):
                    assert 'max_completion_tokens' in body and 'max_tokens' not in body
        print('PASS real application connection jobs through Chat Completions, Responses and Anthropic Messages')
        # Pricing preview is local and sends neither a model request nor a key to a provider.
        before_requests = len(requests)
        quote = post('/api/pricing-preview', dict(endpoint='https://api.deepseek.com', model='deepseek-v4-flash', pricing={'official_currency':'USD'}))
        assert quote['status']=='official' and quote['currency']=='USD' and quote['rates']['cached'] in (.007,.014)
        assert quote['verified_at']=='2026-09-05' and quote['source'].startswith('https://api-docs.deepseek.com/')
        quote = post('/api/pricing-preview', dict(endpoint='https://proxy.example/v1', model='gpt-5'))
        assert quote['status']=='unknown' and quote['amount'] is None
        post('/api/pricing-preview', dict(endpoint='https://api.openai.com',model='gpt-5',pricing={'mode':'manual','cached_input_per_1k':-1}),expected=400)
        assert len(requests)==before_requests
        config=get('/api/bootstrap')['config']
        config['llm'].update(api_format='chat_completions',model='mimo-v2.5')
        config['llm']['pricing']={'mode':'manual','currency':'CNY','input_per_1k_usd':.002,'output_per_1k_usd':.008}
        post('/api/action', dict(action='config',config=config))
        current=get('/api/tasks/'+task['id'])
        post('/api/action',dict(action='test_connection',task_id=task['id'],revision=current['revision']))
        for _ in range(150):
            state=get('/api/bootstrap')
            if not state['job']:break
            time.sleep(.05)
        assert state['last_job']['status']=='completed',state['last_job']
        recorded=get('/api/tasks/'+task['id'])['calls'][-1]
        assert recorded['billing']['status']=='manual' and recorded['billing']['currency']=='CNY'
        assert abs(recorded['billing']['amount']-.000036)<1e-12 and recorded['cost_usd'] is None
        config['llm']['pricing']['input_per_1k_usd']=900
        post('/api/action',dict(action='config',config=config))
        assert get('/api/tasks/'+task['id'])['calls'][-1]==recorded
        summary=next(t for t in get('/api/tasks') if t['id']==task['id'])
        assert abs(summary['costs']['currencies']['CNY']-.000036)<1e-12
        assert summary['costs']['unknown']>=3
        config['llm']['pricing']={'mode':'auto'}
        post('/api/action',dict(action='config',config=config))
        print('PASS official/manual pricing, currency separation, unknowns, validation and immutable historical charges')

        if args.serve:
            # Restore a helpful fixture model for interactive settings inspection.
            config = get('/api/bootstrap')['config']
            config['llm'].update(model='deepseek-v4-flash', api_format='chat_completions')
            post('/api/action', dict(action='config', config=config))
            print('UI fixture: ' + base, flush=True)
            while True:
                time.sleep(1)
    finally:
        process.terminate()
        process.wait(timeout=10)
        mock.shutdown()
        log.close()


if __name__ == '__main__':
    main()
