//! Build script for Cordial's llama.cpp bindings.
//!
//! Responsibilities, in order:
//!   1. obtain a pinned llama.cpp source tree (download + verify, or use
//!      `CORDIAL_LLAMA_SRC` when a distro/CI has already unpacked one),
//!   2. configure and build static `llama`, `ggml` and `mtmd` libraries with
//!      the backend that matches the target platform,
//!   3. compile the small C++ shim that is Cordial's only unsafe boundary,
//!   4. emit link directives.
//!
//! Knobs (all optional):
//!   CORDIAL_LLAMA_SRC=<dir>   use this llama.cpp checkout/tarball root
//!   CORDIAL_PORTABLE_CPU=1    build without -march=native (distributable)
//!   CORDIAL_GGML_OPENCL=1     force the OpenCL backend (implied on Windows ARM64)
//!   CORDIAL_GGML_VULKAN=1     force the Vulkan backend
//!   CORDIAL_GGML_CUDA=1       force the CUDA backend
//!   CMAKE=<path>              cmake binary to use

use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// llama.cpp release Cordial is pinned to. b12020 is the first release in the
/// b120xx line with the fixes LFM3-VL needs on top of b11960 (minimum noted by
/// the model card). Bump this together with `LLAMA_SHA256`.
const LLAMA_TAG: &str = "b12020";
const LLAMA_SHA256: &str = "5496c675a36bcba53660a922f26cff75a8d24222d0554d337b450b552a30b502";

// Khronos OpenCL bits, only fetched for Windows builds that use OpenCL.
const OPENCL_HEADERS_SHA: &str = "82f60316cc142e7b21dc1d8983b26de18858fbd9";
const OPENCL_ICD_SHA: &str = "007c8e892f52e9b9f288830cbce30ab2f97760d0";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=csrc/cordial_llama.cpp");
    println!("cargo:rerun-if-changed=csrc/cordial_llama.h");
    for key in [
        "CORDIAL_LLAMA_SRC",
        "CORDIAL_PORTABLE_CPU",
        "CORDIAL_GGML_OPENCL",
        "CORDIAL_GGML_VULKAN",
        "CORDIAL_GGML_CUDA",
        "CMAKE",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let target = env::var("TARGET").unwrap_or_default();
    let is_windows = target.contains("windows");
    let is_apple = target.contains("apple");
    let is_msvc = target.contains("msvc");

    let src = resolve_source(&out_dir);

    let want_opencl = flag("CORDIAL_GGML_OPENCL")
        || cfg!(feature = "opencl")
        || target.contains("aarch64-pc-windows");
    let want_vulkan = flag("CORDIAL_GGML_VULKAN") || cfg!(feature = "vulkan");
    let want_cuda = flag("CORDIAL_GGML_CUDA") || cfg!(feature = "cuda");

    let build_dir = out_dir.join("llama-build");
    configure(
        &src,
        &build_dir,
        &target,
        want_opencl,
        want_vulkan,
        want_cuda,
        &out_dir,
    );
    build(&build_dir);

    link(&build_dir, is_windows, is_apple, is_msvc);
    if want_opencl && is_windows {
        // ggml-opencl is static. Its OpenCL loader dependency and its Windows
        // system libraries must also be passed to Rust's final linker.
        println!("cargo:rustc-link-search=native={}", out_dir.join("opencl-sdk/lib").display());
        println!("cargo:rustc-link-lib=static=OpenCL");
        println!("cargo:rustc-link-lib=cfgmgr32");
        println!("cargo:rustc-link-lib=runtimeobject");
    }

    compile_shim(&src, &out_dir);

    let backend = if want_cuda {
        "cuda"
    } else if is_apple {
        "metal"
    } else if want_vulkan {
        "vulkan"
    } else if want_opencl {
        "opencl"
    } else {
        "cpu"
    };
    println!("cargo:rustc-env=CORDIAL_BACKEND={backend}");
    println!("cargo:rustc-env=CORDIAL_LLAMA_TAG={LLAMA_TAG}");
}

fn flag(key: &str) -> bool {
    matches!(
        env::var(key).ok().as_deref(),
        Some("1") | Some("true") | Some("on")
    )
}

// ------------------------------------------------------------------ source

fn resolve_source(out_dir: &Path) -> PathBuf {
    if let Ok(dir) = env::var("CORDIAL_LLAMA_SRC") {
        let path = PathBuf::from(&dir);
        assert!(
            path.join("include").join("llama.h").exists(),
            "CORDIAL_LLAMA_SRC={dir} does not look like a llama.cpp checkout (include/llama.h missing)"
        );
        return path;
    }

    // Download once, reuse across rebuilds: `target/llama-src/<tag>`.
    let cache_root = out_dir
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| out_dir.to_path_buf())
        .join("cordial-llama-cache");
    let src = cache_root.join(format!("llama.cpp-{LLAMA_TAG}"));
    if src.join("include").join("llama.h").exists() {
        return src;
    }

    fs::create_dir_all(&cache_root).expect("create llama cache dir");
    let tarball = cache_root.join(format!("llama.cpp-{LLAMA_TAG}.tar.gz"));
    let url =
        format!("https://codeload.github.com/ggml-org/llama.cpp/tar.gz/refs/tags/{LLAMA_TAG}");
    download_verified(&url, &tarball, Some(LLAMA_SHA256));

    unpack(&tarball, &cache_root);
    assert!(
        src.join("include").join("llama.h").exists(),
        "unpacked llama.cpp source is missing include/llama.h"
    );
    src
}

fn download_verified(url: &str, dest: &Path, sha256: Option<&str>) {
    if dest.exists() {
        if let Some(want) = sha256 {
            if file_sha256(dest).map(|got| got == want).unwrap_or(false) {
                return;
            }
        }
    }

    println!("cargo:warning=downloading {url}");
    let tmp = dest.with_extension("part");
    let resp = ureq::get(url)
        .timeout(Duration::from_secs(900))
        .call()
        .unwrap_or_else(|e| panic!("failed to download {url}: {e}"));
    let mut file = File::create(&tmp).expect("create download file");
    io::copy(&mut resp.into_reader(), &mut file).expect("write download body");
    file.flush().ok();
    drop(file);

    if let Some(want) = sha256 {
        let got = file_sha256(&tmp).expect("hash download");
        assert_eq!(got, want, "checksum mismatch for {url}");
    }
    fs::rename(&tmp, dest).expect("move download into place");
}

fn file_sha256(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn unpack(tarball: &Path, dest: &Path) {
    let file = File::open(tarball).expect("open tarball");
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest).expect("unpack llama.cpp tarball");
}

// ------------------------------------------------------------------- cmake

fn cmake_bin() -> String {
    env::var("CMAKE").unwrap_or_else(|_| "cmake".to_string())
}

fn configure(
    src: &Path,
    build_dir: &Path,
    target: &str,
    opencl: bool,
    vulkan: bool,
    cuda: bool,
    out_dir: &Path,
) {
    let mut cmd = Command::new(cmake_bin());
    cmd.arg("-S")
        .arg(src)
        .arg("-B")
        .arg(build_dir)
        .arg("-DCMAKE_BUILD_TYPE=Release")
        .arg("-DBUILD_SHARED_LIBS=OFF")
        // Library only: no CLI tools, examples, tests or server.
        .arg("-DLLAMA_BUILD_TESTS=OFF")
        .arg("-DLLAMA_BUILD_TOOLS=OFF")
        .arg("-DLLAMA_BUILD_EXAMPLES=OFF")
        .arg("-DLLAMA_BUILD_COMMON=OFF")
        // The unified `llama` binary needs build-info.h from a git checkout.
        .arg("-DLLAMA_BUILD_APP=OFF")
        .arg("-DLLAMA_CURL=OFF")
        .arg("-DLLAMA_BUILD_MTMD=ON")
        .arg("-DGGML_CCACHE=OFF")
        .arg("-DGGML_RPC=OFF");

    if flag("CORDIAL_PORTABLE_CPU") {
        cmd.arg("-DGGML_NATIVE=OFF");
    } else {
        cmd.arg("-DGGML_NATIVE=ON");
    }

    if target.contains("apple") {
        cmd.arg("-DGGML_METAL=ON")
            .arg("-DGGML_METAL_EMBED_LIBRARY=ON");
    }

    if opencl {
        cmd.arg("-DGGML_OPENCL=ON");
        if target.contains("windows") {
            let (include_dir, library) = windows_opencl(out_dir);
            cmd.arg(format!("-DOpenCL_INCLUDE_DIR={}", include_dir.display()))
                .arg(format!("-DOpenCL_LIBRARY={}", library.display()));
        }
    }

    if vulkan {
        cmd.arg("-DGGML_VULKAN=ON");
    }
    if cuda {
        cmd.arg("-DGGML_CUDA=ON");
    }

    run(cmd, "cmake configure");
}

fn windows_opencl(out_dir: &Path) -> (PathBuf, PathBuf) {
    let root = out_dir.join("opencl-sdk");
    let headers = root.join("OpenCL-Headers");
    let loader_src = root.join("OpenCL-ICD-Loader");

    if !headers.join("CL").join("cl.h").exists() {
        fs::create_dir_all(&root).expect("create opencl dir");
        let tarball = root.join("headers.tar.gz");
        download_verified(
            &format!("https://codeload.github.com/KhronosGroup/OpenCL-Headers/tar.gz/{OPENCL_HEADERS_SHA}"),
            &tarball,
            None,
        );
        unpack(&tarball, &root);
        let unpacked = root.join(format!("OpenCL-Headers-{OPENCL_HEADERS_SHA}"));
        if unpacked.exists() && !headers.exists() {
            fs::rename(&unpacked, &headers).expect("rename opencl headers");
        }
    }

    let loader_lib = root.join("lib").join("OpenCL.lib");
    if !loader_lib.exists() {
        fs::create_dir_all(&root).expect("create opencl dir");
        let tarball = root.join("loader.tar.gz");
        download_verified(
            &format!("https://codeload.github.com/KhronosGroup/OpenCL-ICD-Loader/tar.gz/{OPENCL_ICD_SHA}"),
            &tarball,
            None,
        );
        unpack(&tarball, &root);
        let unpacked = root.join(format!("OpenCL-ICD-Loader-{OPENCL_ICD_SHA}"));
        if unpacked.exists() && !loader_src.exists() {
            fs::rename(&unpacked, &loader_src).expect("rename opencl loader");
        }

        let loader_build = root.join("loader-build");
        let mut cmd = Command::new(cmake_bin());
        cmd.arg("-S")
            .arg(&loader_src)
            .arg("-B")
            .arg(&loader_build)
            .arg("-DCMAKE_BUILD_TYPE=Release")
            .arg("-DBUILD_SHARED_LIBS=OFF")
            .arg(format!("-DOPENCL_ICD_LOADER_HEADERS_DIR={}", headers.display()));
        run(cmd, "cmake configure (OpenCL ICD loader)");

        let mut cmd = Command::new(cmake_bin());
        cmd.arg("--build")
            .arg(&loader_build)
            .arg("--config")
            .arg("Release");
        run(cmd, "cmake build (OpenCL ICD loader)");

        if let Some(found) = find_library(&loader_build, &["OpenCL"]) {
            fs::create_dir_all(root.join("lib")).ok();
            fs::copy(&found, &loader_lib).ok();
        }
        assert!(
            loader_lib.exists(),
            "OpenCL ICD loader library was not produced"
        );
    }

    (headers, loader_lib)
}

fn build(build_dir: &Path) {
    let mut cmd = Command::new(cmake_bin());
    cmd.arg("--build")
        .arg(build_dir)
        .arg("--config")
        .arg("Release")
        .arg("--parallel");
    run(cmd, "cmake build");
}

fn run(mut cmd: Command, what: &str) {
    let display = format!("{cmd:?}");
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!(
            "{what} could not start ({e}).\n\
             Cordial compiles llama.cpp from source, so a C++ toolchain and CMake 3.16+ are required.\n\
             Install CMake (https://cmake.org/download) and set CMAKE=/path/to/cmake if it is not on PATH.\n\
             Command: {display}"
        ));
    assert!(
        status.success(),
        "{what} failed with {status}.\nCommand: {display}"
    );
}

// ------------------------------------------------------------------- links

fn link(build_dir: &Path, is_windows: bool, is_apple: bool, is_msvc: bool) {
    // Dependency order matters for single-pass static linkers.
    const ORDER: &[&str] = &[
        "mtmd",
        "llama",
        "ggml",
        "ggml-cpu",
        "ggml-base",
        "vendor-hash",
        "ggml-metal",
        "ggml-blas",
        "ggml-opencl",
        "ggml-vulkan",
        "ggml-cuda",
        "ggml-rpc",
        "ggml-hip",
        "ggml-sycl",
        "ggml-webgpu",
    ];

    let mut search_dirs: Vec<PathBuf> = Vec::new();
    for name in ORDER {
        if let Some(path) = find_library(build_dir, &[*name]) {
            if let Some(dir) = path.parent() {
                if !search_dirs.iter().any(|d| d == dir) {
                    search_dirs.push(dir.to_path_buf());
                }
            }
            println!("cargo:rustc-link-lib=static={name}");
        } else if matches!(*name, "mtmd" | "llama" | "ggml" | "ggml-base" | "ggml-cpu") {
            panic!(
                "required llama.cpp library `{name}` was not produced in {}",
                build_dir.display()
            );
        }
    }
    for dir in &search_dirs {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }

    if is_windows {
        // OpenMP runtime used by ggml-cpu, plus sockets/user env for symbols
        // that static ggml objects reference even when unused at runtime.
        if is_msvc {
            // ggml-cpu compiles with /openmp on MSVC, which needs the vcomp
            // runtime (an import library, not a static archive).
            println!("cargo:rustc-link-lib=vcomp");
        } else {
            println!("cargo:rustc-link-lib=gomp");
        }
        println!("cargo:rustc-link-lib=ws2_32");
        println!("cargo:rustc-link-lib=userenv");
        println!("cargo:rustc-link-lib=bcrypt");
        println!("cargo:rustc-link-lib=advapi32");
    } else if !is_apple {
        // ggml-cpu enables OpenMP when the toolchain provides it, and ggml uses
        // libm and dlopen directly.
        println!("cargo:rustc-link-lib=gomp");
        println!("cargo:rustc-link-lib=dylib=dl");
        println!("cargo:rustc-link-lib=dylib=m");
    }

    if is_apple {
        for fw in [
            "Foundation",
            "Metal",
            "MetalKit",
            "Accelerate",
            "CoreFoundation",
        ] {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
    }
}

/// Finds `lib<name>.a` or `<name>.lib` anywhere under `root`.
fn find_library(root: &Path, names: &[&str]) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let file = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => continue,
            };
            for wanted in names {
                let candidates = [
                    format!("lib{wanted}.a"),
                    format!("{wanted}.lib"),
                    format!("lib{wanted}-static.a"),
                ];
                if candidates.iter().any(|c| c == file) {
                    return Some(path);
                }
            }
        }
    }
    None
}

// -------------------------------------------------------------------- shim

fn compile_shim(src: &Path, out_dir: &Path) {
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .file("csrc/cordial_llama.cpp")
        .include(src.join("include"))
        .include(src.join("ggml").join("include"))
        .include(src.join("tools").join("mtmd"))
        .include(out_dir)
        .define("NDEBUG", None);

    if cfg!(target_env = "msvc") {
        build.flag_if_supported("/EHsc");
    }

    build.compile("cordial_llama_shim");
}
