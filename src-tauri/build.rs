use std::{env, fs, path::Path, process::Command};

fn main() {
    let macos = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if macos {
        stage_llama_dylibs();
        stage_native_stt();
        stage_native_vibe();
        stage_microsoft_runtime();
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

// Keep the additional GGML engine in its own executable: its symbols must never
// be linked into the process already hosting whisper-rs and llama.cpp.
fn stage_native_stt() {
    for path in ["../native-stt", "../scripts/build-native-stt.sh", "tauri.macos.conf.json"] {
        println!("cargo:rerun-if-changed={path}");
    }
    let target = env::var("TARGET").unwrap();
    // Share the Release C++ build across cargo check, clippy, test and app profiles.
    // Cargo's target-directory lock serializes the builds using this cache.
    let output = env::var("OUT_DIR").unwrap();
    let cache = Path::new(&output).ancestors().nth(4).unwrap().join("native-stt").join(&target);
    let status = Command::new("/bin/bash")
        .arg("../scripts/build-native-stt.sh")
        .arg(target)
        .arg(cache)
        .status()
        .expect("Could not start the native speech helper build");
    assert!(status.success(), "Native speech helper build failed");
}

fn stage_native_vibe() {
    for path in ["../native-vibe", "../scripts/build-native-vibe.sh"] { println!("cargo:rerun-if-changed={path}"); }
    let target = env::var("TARGET").unwrap();
    let output = env::var("OUT_DIR").unwrap();
    let cache = Path::new(&output).ancestors().nth(4).unwrap().join("native-vibe").join(&target);
    let status = Command::new("/bin/bash").arg("../scripts/build-native-vibe.sh").arg(target).arg(cache).status().expect("Could not build Microsoft BitNet speech helper");
    assert!(status.success(), "Microsoft BitNet speech helper build failed");
}

// PyInstaller runs only for a native app build/dev launch, not Cargo verification in CI.
// Stage its already-built folder beside the development executable when it is available.
fn stage_microsoft_runtime() {
    println!("cargo:rerun-if-changed=microsoft-runtime");
    let source = Path::new("microsoft-runtime");
    // Tauri also resolves bundle resources during `cargo test`/CI. An empty
    // marker lets those checks run without freezing the optional model runtime.
    if !source.is_dir() {
        fs::create_dir_all(source).expect("Could not create Microsoft runtime resource folder");
        fs::write(source.join("UNBUILT.txt"), "Run the app build to package the Microsoft speech runtime.\n").unwrap();
    }
    if !source.join("openglaido-microsoft").is_file() { return; }
    let output = env::var("OUT_DIR").unwrap();
    let profile = Path::new(&output).ancestors().nth(3).unwrap();
    let status = Command::new("/usr/bin/ditto").arg(source).arg(profile.join("microsoft-runtime")).status().expect("Could not stage Microsoft speech runtime");
    assert!(status.success(), "Microsoft speech runtime staging failed");
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
