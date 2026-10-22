use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_widget_bridge();
    }
    tauri_build::build()
}

fn xcrun(args: &[&str]) -> String {
    let out = Command::new("xcrun").args(args).output().expect("xcrun (Xcode command line tools) is required");
    assert!(out.status.success(), "xcrun {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Compiles swift/WidgetBridge.swift into a static library for the target architecture.
fn build_widget_bridge() {
    let src = "swift/WidgetBridge.swift";
    println!("cargo:rerun-if-changed={src}");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    };
    let sdk = xcrun(&["--sdk", "macosx", "--show-sdk-path"]);
    let lib = out.join("libwidgetbridge.a");
    let status = Command::new("xcrun")
        .args(["swiftc", "-parse-as-library", "-emit-library", "-static", "-O", "-module-name", "WidgetBridge"])
        .args(["-target", &format!("{arch}-apple-macos11.0"), "-sdk", &sdk, src, "-o"])
        .arg(&lib)
        .status()
        .expect("failed to run swiftc");
    assert!(status.success(), "swiftc failed to build the widget bridge");

    // Swift runtime: system dylibs (/usr/lib/swift) plus the toolchain's compatibility shims.
    let swiftc = PathBuf::from(xcrun(&["--find", "swiftc"]));
    let toolchain_lib = swiftc.parent().unwrap().parent().unwrap().join("lib/swift/macosx");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=widgetbridge");
    println!("cargo:rustc-link-lib=framework=WidgetKit");
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    println!("cargo:rustc-link-search=native={}", toolchain_lib.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
