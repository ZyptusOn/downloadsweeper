"""Create 500 synthetic trial files, never overwrite an existing destination.

Python standard library only. Run from the project root. No network, macros or
executable software; the bundled MP4 is a generated color test pattern.
"""
from pathlib import Path
from collections import Counter
from xml.sax.saxutils import escape
import argparse
import hashlib
import io
import json
import math
import os
import struct
import time
import wave
import zipfile
import zlib
from demo_pdf import pdf_bytes

ROOT = Path(__file__).resolve().parents[1]


def archive(parts):
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as z:
        for name, content in parts.items():
            z.writestr(name, content)
    return out.getvalue()


def office(kind, label):
    text = escape(label + '：所有内容为合成演示数据，无真实个人信息。')
    ns = 'http://schemas.openxmlformats.org/'
    rel = ns + 'officeDocument/2006/relationships/'
    package = ns + 'package/2006/relationships'
    parts = {}
    if kind == 'docx':
        main = 'word/document.xml'
        mime = 'wordprocessingml.document.main+xml'
        parts[main] = f'<w:document xmlns:w="{ns}wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p><w:sectPr/></w:body></w:document>'
        overrides = {main: mime}
    elif kind == 'xlsx':
        main = 'xl/workbook.xml'
        parts[main] = f'<workbook xmlns="{ns}spreadsheetml/2006/main" xmlns:r="{rel[:-1]}"><sheets><sheet name="合成成绩" sheetId="1" r:id="rId1"/></sheets></workbook>'
        parts['xl/_rels/workbook.xml.rels'] = f'<Relationships xmlns="{package}"><Relationship Id="rId1" Type="{rel}worksheet" Target="worksheets/sheet1.xml"/></Relationships>'
        parts['xl/worksheets/sheet1.xml'] = f'<worksheet xmlns="{ns}spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>{text}</t></is></c><c r="B1" t="inlineStr"><is><t>合成分数</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>演示班级 A</t></is></c><c r="B2"><v>88</v></c></row></sheetData></worksheet>'
        overrides = {main: 'spreadsheetml.sheet.main+xml', 'xl/worksheets/sheet1.xml': 'spreadsheetml.worksheet+xml'}
    else:
        main = 'ppt/presentation.xml'
        parts[main] = f'<p:presentation xmlns:p="{ns}presentationml/2006/main" xmlns:r="{rel[:-1]}"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>'
        parts['ppt/_rels/presentation.xml.rels'] = f'<Relationships xmlns="{package}"><Relationship Id="rId1" Type="{rel}slide" Target="slides/slide1.xml"/></Relationships>'
        parts['ppt/slides/slide1.xml'] = f'<p:sld xmlns:p="{ns}presentationml/2006/main" xmlns:a="{ns}drawingml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Demo"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="500000" y="500000"/><a:ext cx="8000000" cy="3000000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>'
        overrides = {main: 'presentationml.presentation.main+xml', 'ppt/slides/slide1.xml': 'presentationml.slide+xml'}
    parts['_rels/.rels'] = f'<Relationships xmlns="{package}"><Relationship Id="rId1" Type="{rel}officeDocument" Target="{main}"/></Relationships>'
    parts['[Content_Types].xml'] = '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>' + ''.join(f'<Override PartName="/{name}" ContentType="application/vnd.openxmlformats-officedocument.{mime}"/>' for name, mime in overrides.items()) + '</Types>'
    return archive(parts)


def png(index):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    rows = b''.join(b'\0' + bytes(((index * 37 + y) % 256, 120, 220)) * 320 for y in range(200))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 320, 200, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b'')


def wav():
    out = io.BytesIO()
    with wave.open(out, 'wb') as w:
        w.setparams((1, 2, 8000, 8000, 'NONE', 'not compressed'))
        w.writeframes(b''.join(struct.pack('<h', int(1000 * math.sin(2 * math.pi * 440 * i / 8000))) for i in range(8000)))
    return out.getvalue()


def create(destination):
    destination = Path(destination).absolute()
    # No reset/overwrite option: every trial must start in a fresh directory.
    if any(p.is_symlink() or (p.exists() and getattr(p.lstat(), 'st_file_attributes', 0) & 0x400) for p in [destination, *destination.parents]):
        raise ValueError('Destination must not contain links or reparse points')
    destination.mkdir(parents=True, exist_ok=False)
    entries = []
    audio = wav()
    video = (ROOT / 'scripts/fixtures/synthetic.mp4').read_bytes()
    def put(name, payload, age=0, placeholder=False):
        p = destination / name
        p.parent.mkdir(parents=True, exist_ok=True)
        data = payload if isinstance(payload, bytes) else payload.encode('utf-8')
        with p.open('xb') as out:
            out.write(data)
        stamp = time.time() - age * 86400
        os.utime(p, (stamp, stamp))
        entries.append({'path': name, 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest(), 'placeholder': placeholder})
    formats = ['txt', 'md', 'csv', 'json', 'toml', 'html', 'xml', 'rs', 'py', 'css', 'svg', 'png', 'pdf', 'docx', 'xlsx', 'pptx', 'zip', 'wav', 'mp4', 'log']
    topics = ['项目周报', '课程笔记', '旅行记录', '会议纪要', '剪辑素材', '学习资料', '预算演示', '音乐练习', '设计草图', '文件名重生_a8f3']
    def content(ext, title, i):
        if ext in ('docx', 'xlsx', 'pptx'): return office(ext, title)
        if ext == 'pdf': return pdf_bytes(6 if i % 2 else 1)
        if ext == 'png': return png(i)
        if ext == 'wav': return audio
        if ext == 'mp4': return video
        if ext == 'zip': return archive({'素材说明.txt': title + '，合成素材压缩包。'})
        if ext == 'json': return json.dumps({'title': title, 'synthetic': True}, ensure_ascii=False)
        if ext == 'csv': return '班级,合成平均分\n演示一班,88\n演示二班,91\n'
        if ext == 'svg': return f'<svg xmlns="http://www.w3.org/2000/svg" width="320" height="200"><rect width="320" height="200" fill="#249a96"/><text x="20" y="90" fill="white">DEMO {i}</text></svg>'
        if ext == 'html': return '<!doctype html><meta charset="utf-8"><h1>' + title + '</h1><p>静态合成页面，无脚本和网络资源。</p>'
        if ext == 'xml': return '<demo><title>' + title + '</title></demo>'
        if ext == 'toml': return 'title = "' + title + '"\nsynthetic = true\n'
        if ext == 'rs': return '// 合成 Rust 源文件\nfn main() { println!("demo"); }\n'
        if ext == 'py': return '# 合成 Python 源文件，不需运行\nprint("demo")\n'
        if ext == 'css': return '/* 合成样式 */\nbody { color: #225566; }\n'
        return title + '\n这是试用生成器创建的合成内容，不含真实个人资料。\n'
    # 300 loose download files, then 100 files in meaningful existing folders.
    for i in range(300):
        ext = formats[i % len(formats)]
        title = f'{topics[(i // 20) % len(topics)]}_{i + 1:03}'
        put(f'downloads/{title}.{ext}', content(ext, title, i), age=i % 365)
    for i in range(40):
        put(f'downloads/已有文档/工作资料/2026/第{i // 10 + 1}季度/报告_{i + 1:02}.docx', office('docx', f'合成工作报告 {i}'))
    put('downloads/PortableEditor/editor.exe', 'INERT DEMO PLACEHOLDER. Not an executable.', placeholder=True)
    for i in range(19): put(f'downloads/PortableEditor/resources/asset_{i:02}.json', '{}')
    for i in range(30): put(f'downloads/图标素材库/icons/icon_{i:02}.png', png(i))
    for i in range(5): put(f'downloads/旧下载/下载未完成_{i}.crdownload', 'synthetic partial download', age=400)
    for i in range(3): put(f'downloads/旧下载/空白_{i}.txt', b'', age=400)
    put('downloads/旧下载/重复笔记.txt', '同内容的合成重复样本')
    put('downloads/旧下载/重复笔记 (1).txt', '同内容的合成重复样本')
    # Desktop: 70 loose files + 30 nested files. Folders should stay whole.
    for i in range(70):
        ext = ['txt', 'docx', 'xlsx', 'pdf', 'png', 'pptx', 'md'][i % 7]
        title = f'{topics[i % len(topics)]}_{i + 1:02}'
        put(f'desktop/{title}.{ext}', content(ext, title, i))
    for label in ['按班级总分平均分', '按班级各科平均分', '全校学生成绩排名']:
        put(f'desktop/演示中学八年级2026春期末考成绩/{label}.xlsx', office('xlsx', label))
    for i in range(12): put(f'desktop/项目资料/内部/会议纪要_{i:02}.docx', office('docx', f'项目会议 {i}'))
    for i in range(10): put(f'desktop/文档/已有资料_{i:02}.txt', '复用容器中原有的合成文件')
    for i in range(3): put(f'desktop/快捷方式示例/网站_{i}.url', '[InternetShortcut]\nURL=https://example.com\n')
    put('desktop/快捷方式示例/本地入口.lnk', 'INERT DEMO PLACEHOLDER. Not a shortcut.', placeholder=True)
    put('desktop/快捷方式示例/~$演示.docx', 'INERT Office lock placeholder', placeholder=True)
    assert len(entries) == 500
    counts = dict(sorted(Counter(Path(e['path']).suffix for e in entries).items()))
    manifest = {'version': 1, 'synthetic': True, 'file_count': len(entries), 'bytes': sum(e['bytes'] for e in entries), 'extensions': counts, 'files': entries}
    (destination / 'manifest.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding='utf-8')
    (destination / '试用说明.md').write_text('# 合成试用目录\n\n仅 downloads（400 个文件）与 desktop（100 个文件）是扫描目标；清单和本文不计入 500 个样本。\n\n所有数据为合成内容。PDF/PNG/Office/ZIP/WAV/MP4 可解析，MP4 为无声测试图；EXE、LNK、Office 锁定文件是明确标记的惰性占位，不能运行。PPTX 是用于内容抽取的最小 OOXML 样本，不代表复杂 Office 排版兼容性。\n\n下载模式选择 downloads；桌面模式选择 desktop。桌面已有文件夹默认整体保护，可将“文档”设为复用容器。试用生成计划、审查、执行、恢复；不会自动清理。旧下载中有临时、空白和疑似副本候选，没有巨型文件。\n\n恢复后运行 python -B scripts/create_trial.py --verify <本目录>，核对所有原始路径和 SHA-256。新建的空分类目录允许保留。重新开始请生成新目录，不要覆盖本目录。\n', encoding='utf-8')
    return manifest


def verify(folder):
    root = Path(folder).resolve()
    manifest = json.loads((root / 'manifest.json').read_text(encoding='utf-8'))
    failures = []
    for entry in manifest['files']:
        path = root / entry['path']
        if not path.resolve().is_relative_to(root) or not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != entry['sha256']:
            failures.append(entry['path'])
    expected = {e['path'] for e in manifest['files']}
    actual = {p.relative_to(root).as_posix() for name in ('downloads', 'desktop') for p in (root / name).rglob('*') if p.is_file()}
    failures += sorted(actual - expected)
    if failures:
        raise ValueError(f'{len(failures)} missing, changed or extra files: ' + ', '.join(failures[:10]))
    print(f'PASS: {len(expected)} original paths and SHA-256 hashes; no extra files.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, help='New directory; never overwrite')
    parser.add_argument('--verify', type=Path, help='Verify original paths and hashes after restoration')
    args = parser.parse_args()
    if args.verify:
        if args.output: parser.error('Use either --output or --verify')
        verify(args.verify)
    else:
        output = args.output or ROOT / 'artifacts' / time.strftime('trial-500-%Y%m%d-%H%M%S')
        result = create(output)
        print(output.absolute())
        print(json.dumps({k: v for k, v in result.items() if k != 'files'}, ensure_ascii=False))
