use std::io::{Cursor, Write};
pub fn pdf(pages: usize) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        vec![],
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let mut children = Vec::new();
    let colors = [
        "0.8 0.1 0.1",
        "0.1 0.6 0.1",
        "0.7 0.2 0.7",
        "0.1 0.2 0.8",
        "0.2 0.7 0.7",
        "0.8 0.6 0.1",
    ];
    for i in 0..pages {
        let page = objects.len() + 1;
        let stream = page + 1;
        children.push(format!("{page} 0 R"));
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 3 0 R >> >> /Contents {stream} 0 R >>").into_bytes());
        let drawing=format!("{} rg 0 0 600 800 re f 1 1 1 rg BT /F1 44 Tf 60 690 Td (PDF PAGE {}) Tj 0 -70 Td /F1 22 Tf (Native preview fixture) Tj ET",colors[i%6],i+1);
        objects.push(
            format!(
                "<< /Length {} >>\nstream\n{drawing}\nendstream",
                drawing.len()
            )
            .into_bytes(),
        );
    }
    objects[1] = format!(
        "<< /Type /Pages /Count {pages} /Kids [{}] >>",
        children.join(" ")
    )
    .into_bytes();
    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        write!(out, "{} 0 obj\n", i + 1).unwrap();
        out.extend(obj);
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).unwrap();
    for offset in offsets {
        writeln!(out, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    out
}
pub fn png() -> Vec<u8> {
    let mut data = Cursor::new(Vec::new());
    image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]))
        .write_to(&mut data, image::ImageFormat::Png)
        .unwrap();
    data.into_inner()
}
pub fn office() -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in [
        (
            "word/document.xml",
            "<document><p><t>OFFICE_ALLOWED: 项目预算计划</t></p></document>",
        ),
        ("word/vbaProject.bin", "NEVER_EXECUTE_MACRO"),
    ] {
        z.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(bytes.as_bytes()).unwrap();
    }
    z.finish().unwrap().into_inner()
}
pub const VIDEO: &[u8] = include_bytes!("../../../../scripts/fixtures/synthetic.mp4");
