//! Synthetic desktop assets. No user's files or conversation payloads are embedded.
use anyhow::Result;
use ds_engine::permission::{AccessTier, PermissionConfig, PermissionRule};
use std::{
    fs,
    io::{Cursor, Write},
    path::Path,
};

pub fn permissions() -> PermissionConfig {
    PermissionConfig {
        default: AccessTier::FilenameOnly,
        content_slice_bytes: 2048,
        rules: vec![
            PermissionRule {
                category: Some("代码与配置".into()),
                extensions: vec!["ini".into()],
                tier: AccessTier::None,
                ..Default::default()
            },
            PermissionRule {
                category: Some("演示内容".into()),
                extensions: [
                    "txt", "md", "log", "docx", "xlsx", "pptx", "pdf", "png", "jpg", "mp4",
                    "@folder",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
                tier: AccessTier::ContentSlice,
                ..Default::default()
            },
        ],
    }
}
fn put(root: &Path, name: &str, bytes: impl AsRef<[u8]>) -> Result<()> {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, bytes)?;
    Ok(())
}
fn office(ext: &str, text: &str) -> Result<Vec<u8>> {
    let (part,kind,relationship,body)=match ext {
        "docx"=>("word/document.xml","application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml","officeDocument",format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#)),
        "xlsx"=>("xl/workbook.xml","application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml","officeDocument",r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="演示数据" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_owned()),
        _=>("ppt/presentation.xml","application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml","officeDocument",r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#.to_owned()),
    };
    let mut parts = vec![
        (part.to_string(), body),
        (
            "_rels/.rels".into(),
            format!(
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{relationship}" Target="{part}"/></Relationships>"#
            ),
        ),
    ];
    let extra = if ext == "xlsx" {
        parts.push(("xl/_rels/workbook.xml.rels".into(),r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.into()));
        parts.push(("xl/worksheets/sheet1.xml".into(),format!(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>{text}</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>合成演示数据，不含真实成绩或账单</t></is></c><c r="B2"><v>95</v></c></row></sheetData></worksheet>"#)));
        r#"<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
    } else if ext == "pptx" {
        parts.push(("ppt/_rels/presentation.xml.rels".into(),r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#.into()));
        parts.push(("ppt/slides/slide1.xml".into(),format!(r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#)));
        r#"<Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#
    } else {
        ""
    };
    parts.push(("[Content_Types].xml".into(),format!(r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/{part}" ContentType="{kind}"/>{extra}</Types>"#)));
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in parts {
        z.start_file(name, zip::write::SimpleFileOptions::default())?;
        z.write_all(body.as_bytes())?;
    }
    Ok(z.finish()?.into_inner())
}
pub fn pdf() -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        vec![],
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let mut children = vec![];
    for i in 0..3 {
        let p = objects.len() + 1;
        children.push(format!("{p} 0 R"));
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",p+1).into_bytes());
        let drawing=format!("0.1 0.3 0.35 rg 0 0 600 800 re f 1 1 1 rg BT /F1 34 Tf 45 710 Td (DownloadSweeper Demo) Tj 0 -65 Td /F1 22 Tf (Research notes - page {}) Tj 0 -50 Td (Synthetic PDF / no personal content) Tj ET",i+1);
        objects.push(
            format!(
                "<< /Length {} >>\nstream\n{drawing}\nendstream",
                drawing.len()
            )
            .into_bytes(),
        );
    }
    objects[1] = format!("<< /Type /Pages /Count 3 /Kids [{}] >>", children.join(" ")).into_bytes();
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![];
    for (i, v) in objects.iter().enumerate() {
        offsets.push(out.len());
        write!(out, "{} 0 obj\n", i + 1).unwrap();
        out.extend(v);
        out.extend(b"\nendobj\n");
    }
    let start = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).unwrap();
    for n in offsets {
        writeln!(out, "{n:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    out
}
pub fn create(root: &Path) -> Result<()> {
    anyhow::ensure!(!root.exists(), "演示目录必须为全新路径");
    fs::create_dir_all(root)?;
    for (name, text) in [
        (
            "DownloadSweeper 设计文档（演示）.docx",
            "课程设计：Rust Agent 文件整理，权限与恢复设计。",
        ),
        (
            "progress1.docx",
            "本周开发进度：并行规划、多模态取样与操作恢复。",
        ),
        ("AI 开发开销明细（演示）.xlsx", "课程项目开发费用示例"),
        ("新建演示文稿.pptx", "DownloadSweeper 两分钟功能演示"),
        (
            "示例中学八年级2026春期末成绩/按班级总分平均分.xlsx",
            "按班级统计成绩；合成数据",
        ),
        (
            "示例中学八年级2026春期末成绩/按班级各科平均分.xlsx",
            "各科平均分；合成数据",
        ),
        (
            "示例中学八年级2026春期末成绩/全校学生成绩排名.xlsx",
            "成绩排名；合成数据",
        ),
    ] {
        put(root, name, office(name.rsplit('.').next().unwrap(), text)?)?;
    }
    for (name, text) in [
        (
            "Rust 课程笔记.md",
            "# Rust 课程\n所有权、并发任务和取消令牌。演示文本。",
        ),
        ("待办事项.txt", "准备课程演示，整理视频与音乐素材。"),
        (
            "a8f3d92c.txt",
            "标题：Rust Agent 课程答辩提纲\n流程：扫描、权限、目录树、计划、审查、执行。",
        ),
        (
            "render.log",
            "演示渲染缓存日志，可在演示中移入回收站并恢复。",
        ),
        ("空白草稿.txt", ""),
        ("个人设置.ini", "[demo]\nprivacy=synthetic\n"),
        (
            "课程资料库/已归档笔记.md",
            "既有资料容器中的文件，应保留原位。",
        ),
        (
            "便携工具箱/使用说明.txt",
            "合成软件目录，应整体移动；不包含真实可执行文件。",
        ),
        ("便携工具箱/settings.ini", "[demo]\nlayout=compact\n"),
        (
            "otomads/剪辑工程说明.txt",
            "音乐与视频剪辑工程资源，保持整体。",
        ),
        (
            "演示网站.url",
            "[InternetShortcut]\nURL=https://example.com\n",
        ),
    ] {
        put(root, name, text)?;
    }
    put(root, "2603.28759v2（演示）.pdf", pdf())?;
    for (i, name) in [
        "IMG_20240713_170755.jpg",
        "Togawa_sakiko_00006_.png",
        "项目结构截图.png",
    ]
    .iter()
    .enumerate()
    {
        let img = image::RgbImage::from_fn(640, 360, |x, y| {
            image::Rgb([
                ((x / 3 + i as u32 * 50) % 255) as u8,
                ((y / 2 + 60) % 255) as u8,
                140,
            ])
        });
        img.save(root.join(name))?;
    }
    for name in ["FSG1.mp4", "VID_20260529_161156.mp4"] {
        put(
            root,
            name,
            include_bytes!("../../../scripts/fixtures/synthetic.mp4"),
        )?;
    }
    let samples = 8000u32;
    let mut wav = vec![];
    wav.extend(b"RIFF");
    wav.extend((36 + samples * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(8000u32.to_le_bytes());
    wav.extend(16000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((samples * 2).to_le_bytes());
    wav.resize(44 + samples as usize * 2, 0);
    for name in ["oracle2.wav", "Project_3.wav"] {
        put(root, name, &wav)?;
    }
    // Rapid creation can leave NTFS directory-enumeration timestamps a millisecond
    // behind handle metadata. Publish explicit stable times after every child is closed.
    let stamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_780_000_000);
    for item in walkdir::WalkDir::new(root).contents_first(true) {
        let item = item?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.access_mode(0x100).custom_flags(0x02000000);
        }
        options
            .open(item.path())?
            .set_times(fs::FileTimes::new().set_modified(stamp))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures_are_valid_and_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("desktop");
        create(&root).unwrap();
        assert!(create(&root).is_err());
        let mut file = fs::File::open(root.join("progress1.docx")).unwrap();
        let result = ds_engine::evidence::office_excerpt(&mut file, "docx", 1024);
        assert!(result.to_string().contains("本周开发进度"), "{result}");
        assert!(image::open(root.join("IMG_20240713_170755.jpg")).is_ok());
    }
}
