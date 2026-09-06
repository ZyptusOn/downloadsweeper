"""Small, deterministic test PDF; no third-party generator or document application."""
import zlib

def pdf_bytes(pages=6):
    objects=[b'<< /Type /Catalog /Pages 2 0 R >>',b'',b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>']
    children=[]
    colors=['0.8 0.1 0.1','0.1 0.6 0.1','0.7 0.2 0.7','0.1 0.2 0.8','0.2 0.7 0.7','0.8 0.6 0.1']
    for i in range(pages):
        page=len(objects)+1; stream=page+1; children.append(f'{page} 0 R')
        objects.append(f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 3 0 R >> >> /Contents {stream} 0 R >>'.encode())
        drawing=f'{colors[i%6]} rg 0 0 600 800 re f 1 1 1 rg BT /F1 44 Tf 60 690 Td (PDF PAGE {i+1}) Tj 0 -70 Td /F1 22 Tf (Native preview fixture) Tj ET'.encode()
        data=zlib.compress(drawing)
        objects.append(f'<< /Length {len(data)} /Filter /FlateDecode >>\nstream\n'.encode()+data+b'\nendstream')
    objects[1]=f'<< /Type /Pages /Count {pages} /Kids [{" ".join(children)}] >>'.encode()
    result=bytearray(b'%PDF-1.4\n%\xe2\xe3\xcf\xd3\n'); offsets=[0]
    for i,obj in enumerate(objects,1):
        offsets.append(len(result));result.extend(f'{i} 0 obj\n'.encode()+obj+b'\nendobj\n')
    xref=len(result);result.extend(f'xref\n0 {len(objects)+1}\n0000000000 65535 f \n'.encode())
    for offset in offsets[1:]:result.extend(f'{offset:010} 00000 n \n'.encode())
    result.extend(f'trailer\n<< /Size {len(objects)+1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode())
    return bytes(result)
