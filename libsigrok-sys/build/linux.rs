//! Linux native bootstrap.
//!
//! Bundles libffi + zlib + libusb + libzip + pcre2 + glib2 + libsigrok from
//! source into `$OUT_DIR/prefix/`. The resulting binary's `ldd` shows only
//! the glibc family (libc / libm / libpthread / libdl / libgcc_s / ld-linux
//! + linux-vdso). libiconv comes from glibc; nothing extra needed.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::{common, deps};

pub fn build() {
    common::log(">>> libsigrok-sys: Linux native bundle");
    check_toolchain();

    let ctx = make_ctx();
    deps::build_all(&ctx);
    emit_link_directives(&ctx.prefix);
}

fn check_toolchain() {
    common::require_tools(
        &[
            (&["autoconf"],                  "autoconf"),
            (&["automake"],                  "automake"),
            (&["libtoolize", "glibtoolize"], "libtool"),
            (&["pkg-config", "pkgconf"],     "pkg-config / pkgconf"),
            (&["cmake"],                     "cmake"),
            (&["ninja", "ninja-build"],      "ninja"),
            (&["meson"],                     "meson (glib2 build)"),
            (&["python3", "python"],         "python3 (meson runtime)"),
            (&["make", "gmake"],             "make"),
            (&["cc", "gcc", "clang"],        "C compiler"),
            (&["curl"],                      "curl"),
            (&["tar"],                       "tar"),
            (&["bzip2"],                     "bzip2 (libusb .tar.bz2)"),
            (&["xz"],                        "xz (glib .tar.xz)"),
            (&["sed"],                       "sed"),
        ],
        "Arch:   sudo pacman -S base-devel cmake ninja meson curl tar xz bzip2 \\\n\
         \x20                   autoconf automake libtool pkgconf\n\
         Debian: sudo apt install build-essential cmake ninja-build meson curl tar xz-utils bzip2 \\\n\
         \x20                     autoconf automake libtool pkg-config\n\
         Fedora: sudo dnf groupinstall 'Development Tools'\n\
         \x20     sudo dnf install cmake ninja-build meson curl tar xz bzip2 \\\n\
         \x20                      autoconf automake libtool pkgconf-pkg-config",
    );
}

fn make_ctx() -> deps::Ctx {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let prefix = out.join("prefix");
    let src = out.join("src");
    let cache = common::cache_dir();
    let tarballs = cache.join("tarballs");

    fs::create_dir_all(prefix.join("lib/pkgconfig")).expect("mkdir prefix/lib/pkgconfig");
    fs::create_dir_all(prefix.join("include")).expect("mkdir prefix/include");
    fs::create_dir_all(&src).expect("mkdir src");
    fs::create_dir_all(&tarballs).expect("mkdir tarballs");
    common::log(&format!("    cache: {}", cache.display()));

    deps::Ctx {
        prefix,
        src,
        tarballs,
        platform: deps::Platform::Linux,
        cross: None,  // Linux build.rs only supports native Linux targets.
    }
}

// ─── Link directives ─────────────────────────────────────────────────────────

fn emit_link_directives(prefix: &std::path::Path) {
    // Search paths: our bundled prefix. No system paths — we bundled everything
    // libsigrok needs, and want the linker to NOT find stray system .so files.
    println!("cargo:rustc-link-search=native={}", prefix.join("lib").display());

    // glibc family must stay dynamic; static glibc references dynamic-loader
    // symbols (_dl_x86_cpu_features, __libc_single_threaded) and is a known
    // portability footgun.
    let force_dynamic = ["c", "m", "pthread", "dl", "rt", "util", "resolv",
                         "crypt", "nsl", "gcc_s"];

    // Drive link line off `pkg-config --static --libs libsigrok`. libsigrok.pc
    // (post-sed) has Requires.private = "zlib libusb-1.0 gio-2.0 libzip" which
    // recursively expands to glib + pcre2 + ffi + pthread + zlib.
    let out = Command::new("pkg-config")
        .env("PKG_CONFIG_PATH", prefix.join("lib/pkgconfig"))
        .args(["--static", "--libs", "libsigrok"])
        .output()
        .expect("spawn pkg-config");
    if !out.status.success() {
        panic!(
            "pkg-config --static --libs libsigrok failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let libs_line = String::from_utf8(out.stdout).unwrap();

    for token in libs_line.split_whitespace() {
        if let Some(dir) = token.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={dir}");
            continue;
        }
        let Some(name) = token.strip_prefix("-l") else {
            if token.starts_with("-Wl,") || token == "-pthread" {
                println!("cargo:rustc-link-arg={token}");
            }
            continue;
        };
        let has_static = !force_dynamic.contains(&name)
            && prefix.join(format!("lib/lib{name}.a")).exists();
        let kind = if has_static { "static" } else { "dylib" };
        println!("cargo:rustc-link-lib={kind}={name}");
    }
}
