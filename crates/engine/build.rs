fn main() {
    println!("cargo:rerun-if-changed=src/evidence/macos_preview.m");
    println!("cargo:rerun-if-changed=src/evidence/macos_pdf.m");
    println!("cargo:rerun-if-changed=src/recycle/macos.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/evidence/macos_preview.m")
            .file("src/evidence/macos_pdf.m")
            .file("src/recycle/macos.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("ds_macos_preview");
        println!("cargo:rustc-link-lib=objc");
        for framework in [
            "Foundation",
            "AVFoundation",
            "CoreMedia",
            "CoreGraphics",
            "ImageIO",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
