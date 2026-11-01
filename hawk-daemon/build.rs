//! macOS 链接补充 Clang 运行时。
//!
//! ort-sys 下载的 macOS 预编译静态库由 Clang 构建，其中 CoreML 相关对象引用了
//! compiler-rt 提供的 `___isPlatformVersionAtLeast`。Clang 直接链接时会自动带上
//! libclang_rt，但 rustc 调用 `cc` 时固定传 `-nodefaultlibs`，导致最终链接报未定义符号。
//! 这里把 Clang 的运行时静态库显式补进链接参数。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let clang = command_output(Command::new("xcrun").args(["--find", "clang"]));
    let runtime = command_output(Command::new(&clang).arg("-print-libgcc-file-name"));
    let runtime = PathBuf::from(runtime);
    let dir = runtime
        .parent()
        .unwrap_or_else(|| panic!("无法解析 Clang 运行时路径: {}", runtime.display()));
    let file = runtime
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("lib"))
        .and_then(|name| name.strip_suffix(".a"))
        .unwrap_or_else(|| panic!("无法解析 Clang 运行时库名: {}", runtime.display()));

    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static={file}");
}

fn command_output(command: &mut Command) -> String {
    let output = command
        .output()
        .unwrap_or_else(|err| panic!("执行 {command:?} 失败: {err}"));
    assert!(
        output.status.success(),
        "{command:?} 执行失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
