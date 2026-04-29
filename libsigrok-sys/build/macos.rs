//! macOS native bootstrap.
//!
//! Bundles libffi + zlib + libusb + libzip + pcre2 + glib2 + libsigrok from
//! source into `$OUT_DIR/prefix/`. libiconv comes from Darwin libc (BSD iconv).
//! The resulting binary depends only on macOS system frameworks + libSystem.
//!
//! User-side prerequisites (once per machine):
//!
//!   xcode-select --install
//!   brew install autoconf automake libtool pkg-config cmake ninja meson \
//!                curl xz bzip2 gettext

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::{common, deps};

pub fn build() {
    common::log(">>> libsigrok-sys: macOS native bundle");
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
            (&["glibtoolize", "libtoolize"], "libtool (brew install libtool)"),
            (&["pkg-config", "pkgconf"],     "pkg-config / pkgconf"),
            (&["cmake"],                     "cmake"),
            (&["ninja"],                     "ninja"),
            (&["meson"],                     "meson (glib2 build)"),
            (&["python3", "python"],         "python3 (meson runtime)"),
            (&["make", "gmake"],             "make"),
            (&["cc", "clang", "gcc"],        "C compiler (Xcode command-line clang)"),
            (&["curl"],                      "curl"),
            (&["tar"],                       "tar"),
            (&["bzip2"],                     "bzip2 (libusb .tar.bz2)"),
            (&["xz"],                        "xz (glib .tar.xz)"),
            (&["sed"],                       "sed"),
        ],
        "xcode-select --install\n\
         brew install autoconf automake libtool pkg-config cmake ninja meson \\\n\
         \x20           curl xz bzip2",
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
        platform: deps::Platform::MacOS,
        cross: None,  // macOS build.rs only supports native macOS targets.
    }
}

// ─── Link directives ─────────────────────────────────────────────────────────

fn emit_link_directives(prefix: &std::path::Path) {
    println!("cargo:rustc-link-search=native={}", prefix.join("lib").display());

    // Same force_dynamic spirit as Linux, but Darwin's libc family is just
    // "System" (libSystem.dylib) — linked implicitly by the linker. We still
    // emit -lm / -lpthread in case libsigrok.pc mentions them; they'll
    // resolve to libSystem.
    let force_dynamic = ["m", "pthread", "c", "iconv"];

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

    // macOS system frameworks libusb uses for USB access.
    for fw in ["IOKit", "CoreFoundation", "Security"] {
        println!("cargo:rustc-link-lib=framework={fw}");
    }
}
