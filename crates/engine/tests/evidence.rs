use ds_engine::{
    evidence,
    permission::{AccessTier, PermissionConfig},
    workflow::Task,
    workflow_ai,
};
use std::{fs::File, io::Write, path::PathBuf};
use tokio_util::sync::CancellationToken;

fn package(ext: &str, parts: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ds-office-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join(format!("sample.{ext}"));
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    for (name, content) in parts {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    path
}
#[test]
fn office_samples_are_bounded_read_only_and_permission_scoped() {
    let path=package("docx",&[("word/document.xml","<w:document xmlns:w='w'><w:p><w:r><w:t>项目预算 &amp; 计划</w:t></w:r></w:p></w:document>"),("word/vbaProject.bin","DO_NOT_RUN"),("../../outside.txt","DO_NOT_EXTRACT")]);
    let before = std::fs::read(&path).unwrap();
    let mut task = Task::new(
        path.parent().unwrap().to_owned(),
        "organize",
        PermissionConfig {
            default: AccessTier::None,
            rules: vec![],
            content_slice_bytes: 14,
        },
    )
    .unwrap();
    task.scan(&CancellationToken::new(), &|_, _, _| {}).unwrap();
    let entry = task.entries[0].clone();
    assert!(workflow_ai::file_context(&task, &entry).unwrap().is_none());
    for tier in [
        AccessTier::FilenameOnly,
        AccessTier::Metadata,
        AccessTier::Image,
    ] {
        task.permissions.default = tier;
        let context = workflow_ai::file_context(&task, &entry).unwrap().unwrap();
        assert!(context.get("text_excerpt").is_none());
    }
    task.permissions.default = AccessTier::ContentSlice;
    let context = workflow_ai::file_context(&task, &entry).unwrap().unwrap();
    let text = context["text_excerpt"].as_str().unwrap();
    assert!(text.contains("项目预算") && text.len() <= 14);
    assert_eq!(context["content_preview"]["truncated"], true);
    assert!(!context.to_string().contains("DO_NOT_"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    task.permissions.content_slice_bytes = 0;
    assert!(workflow_ai::file_context(&task, &entry)
        .unwrap()
        .unwrap()
        .get("text_excerpt")
        .is_none());
}
#[test]
fn sheets_resolve_shared_strings_and_slides_sample_numbered_parts() {
    let path=package("xlsx",&[("xl/sharedStrings.xml","<sst><si><t>Revenue</t></si><si><t>NOT_REFERENCED</t></si></sst>"),("xl/worksheets/sheet1.xml","<worksheet><row><c t='s'><v>0</v></c><c><f>1+2</f><v>3</v></c><c t='inlineStr'><is><t>Region</t></is></c></row></worksheet>"),("xl/worksheets/sheet3.xml","<worksheet><t>NOT_SAMPLED</t></worksheet>")]);
    let v = evidence::office_excerpt(&mut File::open(path).unwrap(), "xlsx", 128);
    let t = v["text_excerpt"].as_str().unwrap();
    assert!(t.contains("Revenue") && t.contains("3") && t.contains("Region"));
    assert!(!t.contains("NOT_REFERENCED") && !t.contains("1+2"));
    let path = package(
        "pptx",
        &[
            ("ppt/slides/slide10.xml", "<s><t>TEN</t></s>"),
            ("ppt/slides/slide2.xml", "<s><t>TWO</t></s>"),
            ("ppt/slides/slide1.xml", "<s><t>ONE</t></s>"),
            ("ppt/slides/slide3.xml", "<s><t>THREE</t></s>"),
        ],
    );
    let v = evidence::office_excerpt(&mut File::open(path).unwrap(), "pptx", 128);
    assert_eq!(v["text_excerpt"], "ONE TWO THREE ");
}
#[test]
fn corrupt_encrypted_legacy_and_xml_entities_fail_closed() {
    let path = package(
        "docx",
        &[(
            "word/document.xml",
            "<!DOCTYPE a [<!ENTITY x SYSTEM 'file:///private'>]><a><t>&x;</t></a>",
        )],
    );
    assert_eq!(
        evidence::office_excerpt(&mut File::open(&path).unwrap(), "docx", 128)["status"],
        "unavailable"
    );
    assert_eq!(
        evidence::office_excerpt(&mut File::open(&path).unwrap(), "doc", 128)["reason"],
        "legacy_office_unsupported"
    );
    std::fs::write(&path, b"encrypted or corrupt document").unwrap();
    assert_eq!(
        evidence::office_excerpt(&mut File::open(&path).unwrap(), "docx", 128)["status"],
        "unavailable"
    );
    let bomb = format!("<d><t>{}</t></d>", "abc".repeat(1024 * 1024));
    let path = package("docx", &[("word/document.xml", &bomb)]);
    let started = std::time::Instant::now();
    let v = evidence::office_excerpt(&mut File::open(path).unwrap(), "docx", 32);
    assert!(started.elapsed().as_secs() < 3);
    assert!(v["text_excerpt"].as_str().unwrap_or("").len() <= 32);
}
#[test]
fn frame_targets_handle_short_unknown_and_invalid_durations() {
    assert_eq!(evidence::sample_times(Some(100.)), vec![0., 1., 50.]);
    assert_eq!(evidence::sample_times(Some(0.1)), vec![0.]);
    for d in [None, Some(f64::NAN), Some(-1.), Some(f64::INFINITY)] {
        assert_eq!(evidence::sample_times(d), vec![0.]);
    }
}
#[test]
fn image_dialects_do_not_mix_provider_fields() {
    let image = ds_engine::llm::ImageData {
        high_detail: false,
        mime: "image/jpeg".into(),
        data_base64: "/9j/AA==".into(),
    };
    let official = ds_engine::llm::providers::image_url("https://api.openai.com/v1", &image);
    assert_eq!(official["detail"], "low");
    let pdf = ds_engine::llm::ImageData {high_detail:true, ..image.clone()};
    assert_eq!(ds_engine::llm::providers::image_url("https://api.openai.com/v1", &pdf)["detail"], "high");
    assert!(ds_engine::llm::providers::image_url("https://example.com/v1", &pdf).get("detail").is_none());
    let glm = ds_engine::llm::providers::image_url("https://open.bigmodel.cn/api/paas/v4", &image);
    assert_eq!(glm["url"], image.data_base64);
    assert!(glm.get("detail").is_none());
    for host in [
        "api.deepseek.com",
        "api.xiaomimimo.com",
        "api.longcat.chat",
        "tokenhub.tencentmaas.com",
        "proxy.example",
    ] {
        let v = ds_engine::llm::providers::image_url(&format!("https://{host}/v1"), &image);
        assert!(v["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/jpeg;base64,"));
        assert!(v.get("detail").is_none());
    }
}
