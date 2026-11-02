use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    #[cfg(target_os = "macos")]
    compile_macos_stt_helper();

    // Expose host triple so runtime can locate the ffmpeg sidecar in `tauri dev`.
    if let Ok(target) = env::var("TARGET") {
        println!("cargo:rustc-env=TARGET_TRIPLE={target}");
    }
    println!("cargo:rerun-if-changed=binaries");

    tauri_build::build();
}

#[cfg(target_os = "macos")]
fn compile_macos_stt_helper() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest_dir.join("native_stt/macos_transcribe.swift");
    println!("cargo:rerun-if-changed={}", src.display());

    if !src.exists() {
        println!("cargo:warning=native_stt/macos_transcribe.swift missing");
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bin = out_dir.join("macos_transcribe");

    let status = Command::new("swiftc")
        .args([
            "-O",
            "-framework",
            "Speech",
            "-framework",
            "AVFoundation",
            "-o",
        ])
        .arg(&bin)
        .arg(&src)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-env=NATIVE_STT_MACOS_HELPER={}", bin.display());
            // Also copy beside the target dir helpers for runtime discovery
            if let Ok(profile) = env::var("PROFILE") {
                if let Ok(target) = env::var("CARGO_TARGET_DIR")
                    .or_else(|_| env::var("OUT_DIR").map(|_| String::new()))
                {
                    let _ = (profile, target);
                }
            }
        }
        Ok(_) => println!("cargo:warning=swiftc failed building macos_transcribe"),
        Err(e) => println!("cargo:warning=swiftc unavailable ({e}); native STT will compile on first use"),
    }
}
