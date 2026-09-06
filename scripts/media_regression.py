"""Local-only Office/video/protocol regression. Optional DS_TEST_MEDIA_BIN supplies FFmpeg tools."""
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.request import Request, urlopen
import base64, hashlib, json, os, struct, subprocess, sys, threading, time, uuid, zipfile, zlib
from pdf_fixture import pdf_bytes

ROOT=Path(__file__).resolve().parents[1]
class Mock(BaseHTTPRequestHandler):
    calls=[]
    def log_message(self,*args):pass
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert not self.headers.get('Authorization') and not self.headers.get('x-api-key')
        Mock.calls.append(body)
        turns=body.get('messages',body.get('input',[]))
        blocks=turns[-1]['content']
        text=blocks if isinstance(blocks,str) else next(b['text'] for b in blocks if b['type'] in ('text','input_text'))
        context=json.loads(text)
        answer=json.dumps({'name':context['name'],'reason':'fixture retains name'})
        if self.path.endswith('/messages'):
            result={'content':[{'type':'text','text':answer}],'usage':{'input_tokens':100,'output_tokens':10},'stop_reason':'end_turn'}
        elif self.path.endswith('/responses'):
            result={'status':'completed','output':[{'type':'message','content':[{'type':'output_text','text':answer}]}],'usage':{'input_tokens':100,'output_tokens':10}}
        else:result={'choices':[{'message':{'role':'assistant','content':answer},'finish_reason':'stop'}],'usage':{'prompt_tokens':100,'completion_tokens':10}}
        data=json.dumps(result).encode();self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)

def main():
    directory=ROOT/'artifacts'/('media-regression-'+uuid.uuid4().hex[:8]);directory.mkdir()
    files=directory/'files';files.mkdir()
    with zipfile.ZipFile(files/'budget.docx','w',compression=zipfile.ZIP_DEFLATED) as z:
        z.writestr('word/document.xml','<document><p><t>OFFICE_ALLOWED: 项目预算计划</t></p></document>')
        z.writestr('word/vbaProject.bin','NEVER_EXECUTE_MACRO')
    def chunk(kind,data):return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data)&0xffffffff)
    png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',2,2,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(b'\0'+b'\xff\0\0'*2+b'\0'+b'\xff\0\0'*2))+chunk(b'IEND',b'')
    (files/'image.png').write_bytes(png)
    (files/'document.pdf').write_bytes(pdf_bytes())
    native_pdf = os.name == 'nt' or sys.platform == 'darwin'
    env={k:v for k,v in os.environ.items() if 'API_KEY' not in k and not k.startswith('DS_MODEL_KEY_')}
    media_bin=os.environ.get('DS_TEST_MEDIA_BIN')
    fallback = '--ffmpeg-fallback' in sys.argv
    if media_bin:
        env['PATH']=media_bin+os.pathsep+env.get('PATH','')
        subprocess.run([str(Path(media_bin)/('ffmpeg.exe' if os.name == 'nt' else 'ffmpeg')),'-nostdin','-v','error','-f','lavfi','-i','testsrc2=size=320x240:rate=4:duration=6','-c:v','ffv1' if fallback else 'libx264','-threads','1','-g','1',str(files/('video.mkv' if fallback else 'video.mp4'))],check=True,env=env,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
    else:(files/'video.mp4').write_bytes(b'not a decodable video')
    native_only = '--native-only' in sys.argv
    if native_only:
        assert media_bin and (os.name == 'nt' or sys.platform == 'darwin'), 'Native fixture requires Windows/macOS and DS_TEST_MEDIA_BIN for video generation'
        env['PATH'] = str(Path(os.environ['SystemRoot'])/'System32') if os.name == 'nt' else '/usr/bin:/bin'
    before={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in files.iterdir()}
    mock=ThreadingHTTPServer(('127.0.0.1',0),Mock);threading.Thread(target=mock.serve_forever,daemon=True).start()
    config=directory/'config.toml';config.write_text(f'[llm]\nendpoint="http://127.0.0.1:{mock.server_port}/v1"\nmodel="media-fixture"\napi_key_env="DS_TEST_EMPTY"\nmultimodal=true\n',encoding='utf-8')
    log=(directory/'server.log').open('w',encoding='utf-8')
    process=subprocess.Popen([str(ROOT/'target/debug'/('ds-web.exe' if os.name == 'nt' else 'ds-web')),'--port','0','--config',str(config),'--data-dir',str(directory/'data')],cwd=directory,env=env,stdout=log,stderr=log,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
    try:
        base=None
        for _ in range(150):
            for word in (directory/'server.log').read_text(encoding='utf-8').split():
                if word.startswith('http://127.0.0.1:'):base=word.rstrip('/')
            if base:break
            time.sleep(.1)
        assert base
        def get(path):return json.load(urlopen(base+path,timeout=10))
        token=get('/api/bootstrap')['token']
        def post(action,task=None,**args):
            body={'action':action,**args}
            if task:body.update(task_id=task['id'],revision=task['revision'])
            return json.load(urlopen(Request(base+'/api/action',data=json.dumps(body).encode(),headers={'Content-Type':'application/json','x-ds-token':token}),timeout=20))
        def wait(result):
            for _ in range(600):
                state=get('/api/bootstrap')
                if not state['job']:
                    assert state['last_job']['status']=='completed',state['last_job'];return get('/api/tasks/'+result['job']['task_id'])
                time.sleep(.05)
            raise AssertionError('job timeout')
        def run(protocol,tier='content_slice',vision=True):
            cfg=get('/api/bootstrap')['config'];cfg['llm'].update(api_format=protocol,multimodal=vision);post('config',config=cfg)
            task=wait(post('scan',post('create',root=str(files),mode='rename')));task=post('advance',task)
            task=post('permissions',task,permissions={'default':tier,'content_slice_bytes':64,'rules':[]})
            task=post('advance',task);task=post('rename_scope',task,extensions=['docx','png','mp4','mkv','pdf'],web_search=False);task=post('advance',task)
            if 'job' in task:task=wait(task)
            start=len(Mock.calls);task=wait(post('rename',task));assert len(Mock.calls)-start==4,'Preview must not add API requests'
            return Mock.calls[start:],task
        for protocol in ['chat_completions','responses','anthropic']:
            calls,task=run(protocol)
            count=0
            for body in calls:
                turns=body.get('messages',body.get('input',[]));content=turns[-1]['content']
                text=content if isinstance(content,str) else next(b['text'] for b in content if b['type'] in ('text','input_text'))
                context=json.loads(text)
                images=[] if isinstance(content,str) else [b for b in content if b['type'] in ('image','input_image','image_url')]
                if context['name'].endswith('.docx'):
                    assert 'OFFICE_ALLOWED' in context['text_excerpt'] and len(context['text_excerpt'].encode())<=64 and not images
                elif context['name'].endswith('.pdf'):
                    assert len(images)==(1 if native_pdf else 0), context
                    if native_pdf:
                        preview=context['visual_preview']
                        assert preview['sampled_pages']==[1,2,4] and preview['page_count']==6 and not preview['partial'],preview
                        block=images[0]
                        encoded=block['source']['data'] if protocol=='anthropic' else (block['image_url'] if protocol=='responses' else block['image_url']['url']).split(',',1)[1]
                        raw=base64.b64decode(encoded);assert raw.startswith(b'\xff\xd8') and len(raw)<=384*1024
                        (directory/f'{protocol}-pdf.jpg').write_bytes(raw)
                        print('PDF',protocol,'native preview',preview['elapsed_ms'],'ms')
                        count+=1
                elif context['name'].endswith('.png') or media_bin:
                    assert len(images)==1
                    image=images[0]
                    encoded=image['source']['data'] if protocol=='anthropic' else (image['image_url'] if protocol=='responses' else image['image_url']['url']).split(',',1)[1]
                    raw=base64.b64decode(encoded);assert raw.startswith(b'\xff\xd8') and len(raw)<256*1024
                    if context['name'].endswith(('.mp4','.mkv')):
                        preview=context['visual_preview'];assert len(preview['sample_targets_seconds'])==3,preview
                        if native_only: assert preview['backend']==('windows_media' if os.name == 'nt' else 'avfoundation'), preview
                        if fallback: assert preview['backend']=='ffmpeg', preview
                        assert preview['sample_targets_seconds'][-1]==3 and preview['layout']=='row_major_2x2'
                        (directory/f'{protocol}-contact.jpg').write_bytes(raw)
                    count+=1
                else:assert not images
            assert count==(2 if media_bin else 1)+int(native_pdf)
            assert not task['pending_calls'] and not task['operations']
            assert 'NEVER_EXECUTE_MACRO' not in json.dumps(calls)
            print('PASS',protocol,'Office slices, JPEG payloads, image mapping and unchanged request count')
        for tier,vision in [('filename_only',True),('content_slice',False)]:
            calls,_=run('chat_completions',tier,vision)
            serialized=json.dumps(calls);assert 'data:image' not in serialized
            if tier=='filename_only':assert 'OFFICE_ALLOWED' not in serialized
        assert {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in files.iterdir()}==before
        capabilities=get('/api/media-capabilities')
        assert capabilities['office']
        if native_only:
            assert capabilities['native_video']==('windows_media' if os.name == 'nt' else 'avfoundation'), capabilities
            if os.name == 'nt': assert not capabilities['ffmpeg'] and not capabilities['ffprobe'], capabilities
            print('PASS native OS decoding selected; FFmpeg excluded from server PATH')
        print('PASS permission/vision gates and original files unchanged')
        print(directory)
        if '--serve' in __import__('sys').argv:
            print('GUI fixture:',base,flush=True)
            while True:time.sleep(1)
    finally:process.terminate();process.wait(timeout=10);mock.shutdown();log.close()
if __name__=='__main__':main()
