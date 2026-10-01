use std::{env, fs, path::Path, process::Command};

fn main() {
    let macos = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if macos {
        stage_llama_dylibs();
    }
    // With bundle.macOS.frameworks set, this copies libs/*.dylib to target/Frameworks and adds the
    // @executable_path/../Frameworks rpath. That covers both `tauri dev` and the bundled .app.
    let windows_msvc = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows_msvc {
        // tauri-build's resource compiler only links the default manifest into bin targets.
        // The lib test harness also imports Common Controls v6 (rfd/wry); without this it
        // dies in the Windows loader with STATUS_ENTRYPOINT_NOT_FOUND before running tests.
        let attributes = tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        tauri_build::try_build(attributes).expect("Tauri build failed");
        let manifest = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    } else {
        tauri_build::build();
    }
    if macos {
        // `cargo test` binaries run from target/<profile>/deps, where llama-cpp-sys-2 also links the dylibs.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
    }
}

// llama-cpp-2 "dynamic-link" builds libllama/libggml*.dylib. whisper-rs keeps its own ggml static, and two
// static ggml copies crash. bundle.macOS.frameworks needs fixed paths, so copy the dylibs to src-tauri/libs.
fn stage_llama_dylibs() {
    let cmake_dir = env::var("DEP_LLAMA_GGML_CMAKE_DIR").expect("llama-cpp-sys-2 must be a direct dependency");
    let lib_dir = Path::new(&cmake_dir).parent().unwrap();
    fs::create_dir_all("libs").unwrap();
    // Keep in sync with tauri.conf.json > bundle > macOS > frameworks.
    for name in ["llama", "llama-common", "ggml", "ggml-base", "ggml-cpu", "ggml-metal"] {
        let file = format!("lib{name}.0.dylib");
        let src = lib_dir.join(&file);
        let bytes = fs::read(&src).unwrap_or_else(|e| panic!("{}: {e}", src.display()));
        let dst = Path::new("libs").join(&file);
        // Write only on change so rebuilds don't touch the files.
        if fs::read(&dst).ok().as_deref() != Some(bytes.as_slice()) {
            fs::write(&dst, &bytes).unwrap();
        }
        println!("cargo:rerun-if-changed={}", src.display());
    }
    // whisper-rs's static ggml-metal has @available(macOS 15) checks. Below that deployment target they call
    // __isPlatformVersionAtLeast. Rust std defines it, but release LTO drops it, so link clang's builtins.
    let rt = Command::new("cc").arg("-print-libgcc-file-name").output().unwrap().stdout;
    println!("cargo:rustc-link-arg={}", String::from_utf8(rt).unwrap().trim());
}
