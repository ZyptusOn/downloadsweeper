"""Native PDF IPC, real page rendering, snapshot/security bounds. No live LLM."""
from pathlib import Path
import base64,hashlib,json,os,subprocess,sys,time,uuid
from pdf_fixture import pdf_bytes

ROOT=Path(__file__).resolve().parents[1]
def main():
    assert os.name=='nt' or sys.platform=='darwin'
    folder=ROOT/'artifacts'/('pdf-preview-'+uuid.uuid4().hex[:8]);folder.mkdir(parents=True)
    pdf=folder/'页面 样本.pdf';pdf.write_bytes(pdf_bytes())
    (folder/'config.toml').write_text('invalid TOML [')
    (folder/'.env').write_text('invalid environment fixture')
    exe=ROOT/'target/debug'/('ds-web.exe' if os.name=='nt' else 'ds-web')
    env={'SystemRoot':os.environ['SystemRoot']} if os.name=='nt' else {}
    flags=getattr(subprocess,'CREATE_NO_WINDOW',0)
    def invoke(path,stale=False,source=None,snapshot=None):
        st=snapshot or path.stat()
        return subprocess.run([str(exe),'--native-pdf-preview',str(path),str(st.st_size),str(st.st_mtime_ns//1000000+int(stale)*1000)],
            stdin=source,cwd=folder,env=env,capture_output=True,timeout=10,creationflags=flags)
    def render(path,expected):
        before=hashlib.sha256(path.read_bytes()).hexdigest();started=time.monotonic()
        with path.open('rb') as source: result=invoke(path,source=source)
        elapsed=round((time.monotonic()-started)*1000)
        assert result.returncode==0, (result.returncode,result.stdout,result.stderr)
        packets=[json.loads(line) for line in result.stdout.splitlines()]
        visual=packets[-1];info=visual['info']
        assert info['sampled_pages']==expected and not info['partial'],info
        assert info['backend']==('windows_pdf' if os.name=='nt' else 'coregraphics_pdf')
        assert visual['image']['high_detail'] and len(result.stdout)<=3*1024*1024
        raw=base64.b64decode(visual['image']['data_base64']);assert raw.startswith(b'\xff\xd8') and len(raw)<=384*1024
        (folder/(path.stem+'.jpg')).write_bytes(raw)
        assert hashlib.sha256(path.read_bytes()).hexdigest()==before
        assert len(packets)>=len(expected), 'Successful pages should be published before later pages finish'
        print('PASS PDF',len(expected),'sampled pages',elapsed,'ms',len(raw),'JPEG bytes')
    render(pdf,[1,2,4])
    single=folder/'single.pdf';single.write_bytes(pdf_bytes(1));render(single,[1])
    with pdf.open('rb') as source:assert invoke(pdf,stale=True,source=source).returncode!=0
    broken=folder/'broken.pdf';broken.write_bytes(b'%PDF-1.4\nnot a valid document')
    with broken.open('rb') as source:assert invoke(broken,source=source).returncode!=0
    oversized=folder/'oversized.pdf'
    with oversized.open('wb') as out:out.write(b'%PDF-1.4\n');out.truncate(64*1024*1024+1)
    with oversized.open('rb') as source:assert invoke(oversized,source=source).returncode!=0
    if sys.platform=='darwin':
        with pdf.open('rb') as source:
            snapshot=pdf.stat();pdf.rename(folder/'original.pdf');pdf.write_bytes(b'not authorized')
            assert invoke(pdf,source=source,snapshot=snapshot).returncode==0,'Must use inherited descriptor'
    print('PASS native PDF: Unicode path, single-page deduplication, stale/damaged/oversize refusal, no config loading')
    print(folder)
if __name__=='__main__':main()
