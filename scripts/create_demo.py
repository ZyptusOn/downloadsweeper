"""Create disposable demo files under artifacts, never in the user's Downloads."""
from pathlib import Path
import json
import argparse
import struct
import zlib
import zipfile
import io

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--desktop', action='store_true', help='Create a shallow desktop fixture with protected folders')
args = parser.parse_args()

root = Path(__file__).resolve().parents[1]
demo = root / "artifacts" / ("demo-desktop" if args.desktop else "demo-downloads")
demo.mkdir(parents=True, exist_ok=True)
files = {
    "项目周报.pdf": "demo report",
    "课程笔记.txt": "Rust ownership and borrowing study notes",
    "电影预告.mp4": "demo video",
    "设计草图.png": "demo image",
    "财务报表.xlsx": "private spreadsheet",
    "文档/工作资料/已整理合同.pdf": "existing organized document",
    "PortableEditor/editor.exe": "demo executable - not a real program",
    "PortableEditor/config/settings.json": "{}",
    "素材包/说明.txt": "素材说明",
    "素材包/片段01.mp4": "demo clip",
}
if args.desktop:
    def document():
        output = io.BytesIO()
        with zipfile.ZipFile(output, 'w', zipfile.ZIP_DEFLATED) as archive:
            archive.writestr('[Content_Types].xml', '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>')
            archive.writestr('_rels/.rels', '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>')
            archive.writestr('word/document.xml', '<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>本周工作：完成桌面整理演示；文件夹保持完整。</w:t></w:r></w:p></w:body></w:document>')
        return output.getvalue()

    def pdf():
        stream = b'BT /F1 18 Tf 40 140 Td (Desktop organization demo) Tj ET'
        objects = [b'<</Type /Catalog /Pages 2 0 R>>', b'<</Type /Pages /Kids [3 0 R] /Count 1>>',
                   b'<</Type /Page /Parent 2 0 R /MediaBox [0 0 360 200] /Resources <</Font <</F1 4 0 R>>>> /Contents 5 0 R>>',
                   b'<</Type /Font /Subtype /Type1 /BaseFont /Helvetica>>',
                   b'<</Length '+str(len(stream)).encode()+b'>>\nstream\n'+stream+b'\nendstream']
        data = b'%PDF-1.4\n'; offsets = [0]
        for index, obj in enumerate(objects, 1):
            offsets.append(len(data)); data += f'{index} 0 obj\n'.encode()+obj+b'\nendobj\n'
        offset = len(data)
        data += b'xref\n0 6\n0000000000 65535 f \n'
        data += b''.join(f'{n:010} 00000 n \n'.encode() for n in offsets[1:])
        return data + f'trailer\n<</Size 6 /Root 1 0 R>>\nstartxref\n{offset}\n%%EOF\n'.encode()

    def png():
        def chunk(kind, content):
            return struct.pack('>I', len(content))+kind+content+struct.pack('>I', zlib.crc32(kind+content))
        rows = b''.join(b'\0'+bytes((70, 150 if y < 24 else 210, 180))*64 for y in range(48))
        return b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR', struct.pack('>IIBBBBB', 64, 48, 8, 2, 0, 0, 0))+chunk(b'IDAT', zlib.compress(rows))+chunk(b'IEND', b'')

    files = {
        '待办.txt': '演示：整理散落文件，审查后移动，再恢复。',
        '工作周报.docx': document(), '参考资料.pdf': pdf(), '设计截图.png': png(),
        '网站.url': '[InternetShortcut]\nURL=https://example.com\n',
        '~$工作周报.docx': '模拟 Office 锁定文件，不移动',
        '项目资料/内部/会议笔记.txt': '项目文件夹保持完整，不扫描内部内容。',
        '文本/已有笔记.txt': '已有分类也保留原位；新文本进入文本 (2)。',
    }
for relative, content in files.items():
    file = demo / relative
    file.parent.mkdir(parents=True, exist_ok=True)
    if not file.exists():
        file.write_bytes(content if isinstance(content, bytes) else content.encode('utf-8'))
(root / "artifacts/test-config.toml").write_text(
    "scan_root = " + json.dumps(str(demo)) + '\ntoken_budget = 500000\n'
    '[llm]\nendpoint = "http://127.0.0.1:3190/v1"\nmodel = "local-test"\napi_key_env = "DS_DEMO_EMPTY"\n'
    'context_length = 32768\n[permissions]\ndefault = "filename_only"\n'
    'content_slice_bytes = 4096\n', encoding="utf-8")
print(demo)
