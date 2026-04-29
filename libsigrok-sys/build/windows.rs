//! Windows target bootstrap — supports two modes automatically:
//!
//!  * **Native** (`HOST == TARGET`) — inside an MSYS2 UCRT64 shell.
//!    Bundles every C dep from source via `deps::build_all`. Same pattern
//!    as the Linux / macOS native entries; user has pacman-installed the
//!    host toolchain (gcc / autotools / meson / cmake / etc.).
//!
//!  * **Cross** (`HOST != TARGET`, e.g. Linux → Windows) — fetches MSYS2
//!    ucrt64 `.pkg.tar.zst` packages via pacman into a sysroot (all C deps
//!    pre-built: glib2, libusb, libzip, + transitive), then cross-compiles
//!    ONLY libsigrok (Sipeed fork is not in any package repo). Much faster
//!    than re-building everything through a cross toolchain, and avoids
//!    known autoconf/cmake cross issues (libiconv mbrtowc prototype, …).
//!
//! Both paths produce the same single self-contained `.exe`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{common, deps, msys2};

pub fn build() {
    let host   = env::var("HOST").unwrap_or_default();
    let target = env::var("TARGET").unwrap_or_default();
    let is_cross = host != target;

    if is_cross {
        common::log(&format!(">>> libsigrok-sys: Linux → Windows cross ({target})"));
        build_cross(&target);
    } else {
        common::log(">>> libsigrok-sys: Windows native bundle (MSYS2 UCRT64)");
        build_native();
    }
}

// ─── Native (MSYS2 UCRT64 on Windows) ────────────────────────────────────────

fn build_native() {
    check_toolchain_native();

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

    let ctx = deps::Ctx {
        prefix: prefix.clone(),
        src,
        tarballs,
        platform: deps::Platform::Windows,
        cross: None,
    };
    deps::build_all(&ctx);
    emit_link(&prefix);
}

fn check_toolchain_native() {
    common::require_tools(
        &[
            (&["autoconf"],                  "autoconf"),
            (&["automake"],                  "automake"),
            (&["libtoolize", "glibtoolize"], "libtool"),
            (&["pkg-config", "pkgconf"],     "pkg-config / pkgconf"),
            (&["cmake"],                     "cmake"),
            (&["ninja"],                     "ninja"),
            (&["meson"],                     "meson"),
            (&["python3", "python"],         "python3"),
            (&["make", "mingw32-make"],      "make"),
            (&["gcc", "cc", "clang"],        "C compiler (mingw-w64 gcc)"),
            (&["ar"],                        "ar"),
            (&["curl"],                      "curl"),
            (&["tar"],                       "tar"),
            (&["bzip2"],                     "bzip2"),
            (&["xz"],                        "xz"),
            (&["sed"],                       "sed"),
            (&["bash"],                      "bash (libsigrok autogen.sh)"),
        ],
        "Open an MSYS2 UCRT64 shell and run:\n\
         \x20 pacman -S --needed mingw-w64-ucrt-x86_64-{gcc,rust,cmake,ninja,meson,\\\n\
         \x20                                          pkgconf,python} \\\n\
         \x20                   autoconf automake libtool make curl tar xz bzip2 sed git patch",
    );
}

// ─── Cross (Linux host → Windows target) ─────────────────────────────────────

fn build_cross(target: &str) {
    // Cross supports both gnu and gnullvm. Strategy:
    //   gnullvm  → llvm-mingw clang (UCRT), GNU ld.bfd for libtool partial
    //              link (ld.lld rejects -r in COFF mode).
    //   gnu      → Arch mingw-w64-gcc (MSVCRT), native GNU ld everywhere.
    if !target.contains("windows") {
        panic!("libsigrok-sys::windows called for non-Windows target `{target}`");
    }

    let use_llvm = target.contains("gnullvm");
    if use_llvm {
        autodetect_llvm_mingw();
    }
    let (cc, partial_ld) = check_toolchain_cross(use_llvm);

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let sysroot   = out.join("sysroot");
    let sr_prefix = out.join("sr-prefix");
    let cache     = common::cache_dir();
    fs::create_dir_all(&cache).expect("mkdir cache");
    common::log(&format!("    cache:      {}", cache.display()));
    common::log(&format!("    sysroot:    {}", sysroot.display()));
    common::log(&format!("    CC        = {cc}"));
    common::log(&format!("    partial LD= {partial_ld}"));

    msys2::bootstrap(&sysroot, &cache);
    build_libsigrok_cross(&sysroot, &sr_prefix, &cache, &cc, &partial_ld);
    emit_link_cross(&sysroot, &sr_prefix);
}

fn autodetect_llvm_mingw() {
    if common::on_path("x86_64-w64-mingw32-clang") { return; }
    for path in &[
        "/opt/llvm-mingw-ucrt/bin",
        "/opt/llvm-mingw/bin",
        "/opt/llvm-mingw/llvm-mingw-ucrt/bin",
    ] {
        let clang = std::path::Path::new(path).join("x86_64-w64-mingw32-clang");
        if clang.is_file() {
            let cur = env::var("PATH").unwrap_or_default();
            env::set_var("PATH", format!("{path}:{cur}"));
            common::log(&format!("    llvm-mingw auto-detected: {path}"));
            return;
        }
    }
}

/// Verify toolchain on PATH. Returns (CC name, partial-link ld path).
///
/// For gnullvm we need two linkers: clang's ld.lld for normal linking
/// (UCRT-aware) and a GNU ld.bfd for libtool's partial-link step (which
/// uses `-r`, unsupported by ld.lld in COFF mode). Both are exposed to
/// libtool via the `LD` env var on the libsigrok configure call.
fn check_toolchain_cross(use_llvm: bool) -> (String, String) {
    common::require_tools(
        &[
            (&["pacman"],                     "pacman (MSYS2 package resolver)"),
            (&["fakeroot"],                   "fakeroot (pacman uid bypass)"),
            (&["autoconf"],                   "autoconf"),
            (&["automake"],                   "automake"),
            (&["libtoolize", "glibtoolize"],  "libtool"),
            (&["pkg-config", "pkgconf"],      "pkg-config / pkgconf"),
            (&["make", "gmake"],              "make"),
            (&["curl"],                       "curl"),
            (&["tar"],                        "tar"),
            (&["sed"],                        "sed"),
            (&["bash"],                       "bash"),
            (&["x86_64-w64-mingw32-ld.bfd",
               "x86_64-w64-mingw32-ld"],      "mingw-w64 GNU ld (libtool -r partial link)"),
        ],
        "Linux → Windows cross requires driver tools + a GNU mingw-w64 ld (binutils).\n\
         Arch:    sudo pacman -S mingw-w64-binutils pacman fakeroot autoconf automake \\\n\
         \x20                    libtool pkgconf make curl tar sed bash\n\
         Debian:  sudo apt install mingw-w64 pacman-package-manager fakeroot \\\n\
         \x20                      autoconf automake libtool pkg-config make curl tar sed bash",
    );

    // Additional compiler check depends on target flavor.
    let cc = if use_llvm {
        if !common::on_path("x86_64-w64-mingw32-clang") {
            panic!(
                "\n`x86_64-w64-mingw32-clang` not on PATH (needed for `{target}`).\n\
                 Install llvm-mingw:\n\
                 \x20 Arch (AUR): yay -S llvm-mingw-w64-toolchain-ucrt-bin\n\
                 \x20 Or download https://github.com/mstorsjo/llvm-mingw/releases\n\
                 \x20 and add its bin/ to PATH.\n",
                target = env::var("TARGET").unwrap_or_default()
            );
        }
        "x86_64-w64-mingw32-clang".to_string()
    } else {
        if !common::on_path("x86_64-w64-mingw32-gcc") {
            panic!(
                "`x86_64-w64-mingw32-gcc` not on PATH. \
                 Install Arch's `mingw-w64-gcc` or Debian's `mingw-w64`."
            );
        }
        "x86_64-w64-mingw32-gcc".to_string()
    };

    // Locate a GNU ld.bfd. Some systems install it as `-ld`, others as
    // `-ld.bfd`. Reject the one that turns out to be an ld.lld symlink
    // (llvm-mingw's wrapper), since it doesn't support PE -r.
    let partial_ld = pick_gnu_ld().expect(
        "no GNU mingw-w64 ld found. Install binutils' mingw-w64 ld:\n\
         \x20 Arch:   pacman -S mingw-w64-binutils\n\
         \x20 Debian: apt install mingw-w64",
    );

    (cc, partial_ld)
}

fn pick_gnu_ld() -> Option<String> {
    for path in &[
        "/usr/bin/x86_64-w64-mingw32-ld.bfd",
        "/usr/bin/x86_64-w64-mingw32-ld",
    ] {
        if !std::path::Path::new(path).is_file() { continue; }
        // Reject llvm-mingw's ld wrapper (ld.lld in disguise).
        let Ok(real) = std::fs::canonicalize(path) else { continue; };
        let tail = real.file_name()?.to_string_lossy().into_owned();
        if tail.contains("lld") || tail.contains("wrapper") { continue; }
        return Some((*path).to_string());
    }
    None
}

fn build_libsigrok_cross(sysroot: &Path, sr_prefix: &Path, cache: &Path, cc: &str, partial_ld: &str) {
    let sentinel = sr_prefix.join("lib/libsigrok.a");
    if sentinel.exists() {
        common::log(">>> libsigrok cross cached");
        return;
    }
    common::log(">>> libsigrok (cross)");

    let commit = common::libsigrok_commit();
    common::log(&format!("    commit: {commit}"));

    let tarballs = cache.join("tarballs");
    fs::create_dir_all(&tarballs).expect("mkdir tarballs");
    let tarball = tarballs.join(format!("libsigrok-{commit}.tar.gz"));
    common::fetch(&common::libsigrok_tarball_url(&commit), &tarball);

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let src_parent = out.join("src-cross");
    let src_dir = src_parent.join(format!("libsigrok-{commit}"));
    let _ = fs::remove_dir_all(&src_dir);
    fs::create_dir_all(&src_parent).expect("mkdir src_parent");
    common::extract(&tarball, &src_parent);

    // VXI-11 patch — drop scpi_vxi_dev so --gc-sections can strip libtirpc.
    common::run(
        Command::new("sed").current_dir(&src_dir)
            .args(["-i", "/scpi_vxi_dev/d", "src/scpi/scpi.c"]),
        "libsigrok: patch out VXI-11");

    common::run(Command::new("./autogen.sh").current_dir(&src_dir),
                "libsigrok: autogen");

    let ucrt = sysroot.join("ucrt64");
    // -ffunction-sections etc: linker can --gc-sections drop unused code.
    // -DGLIB_STATIC_COMPILATION etc: GLib headers skip __declspec(dllimport)
    //   so we can static-link the .a archives. -DPCRE2_STATIC / -DZIP_STATIC
    //   same idea for those libraries.
    // No explicit -I / -L here: pkg-config injects them during configure.
    let cflags = "-ffunction-sections -fdata-sections -O2 \
                  -DGLIB_STATIC_COMPILATION -DGIO_STATIC_COMPILATION \
                  -DGOBJECT_STATIC_COMPILATION -DGMODULE_STATIC_COMPILATION \
                  -DPCRE2_STATIC -DZIP_STATIC".to_string();
    let pc_path = ucrt.join("lib/pkgconfig");

    common::run(
        Command::new("./configure").current_dir(&src_dir)
            .env("CC", cc)
            // LD is used by libtool's partial-link (reload) step only.
            // Our cc (possibly clang+ld.lld) might not support `-r` on COFF,
            // so force libtool to use a GNU ld.bfd for that specific step.
            .env("LD", partial_ld)
            // MSYS2 .pc files use ${pcfiledir}-relative paths that already
            // resolve inside our sysroot. Do NOT set PKG_CONFIG_SYSROOT_DIR —
            // it would double-prepend the sysroot and break -I/-L paths.
            .env("PKG_CONFIG_PATH",   &pc_path)
            .env("PKG_CONFIG_LIBDIR", &pc_path)
            .env("CFLAGS",  &cflags)
            .arg("--host=x86_64-w64-mingw32")
            .arg(format!("--prefix={}", sr_prefix.display()))
            .arg("--disable-all-drivers").arg("--enable-sipeed-slogic-analyzer")
            .arg("--disable-bindings")
            .arg("--enable-static").arg("--disable-shared")
            .arg("--without-libserialport").arg("--without-libftdi")
            .arg("--without-libhidapi").arg("--without-libbluez")
            .arg("--without-libnettle")
            .arg("--without-librevisa").arg("--without-libgpib").arg("--without-libieee1284"),
        "libsigrok: configure");
    common::run(
        Command::new("make").current_dir(&src_dir)
            .env("CC", cc)
            .args(["-j", &common::num_jobs()]),
        "libsigrok: make");
    common::run(
        Command::new("make").current_dir(&src_dir).arg("install"),
        "libsigrok: install");

    if !sentinel.exists() {
        panic!("libsigrok.a not produced at {}", sentinel.display());
    }

    // Post-install surgery: drop VXI transport objects.
    let _ = Command::new("x86_64-w64-mingw32-ar")
        .current_dir(sr_prefix.join("lib"))
        .args(["d", "libsigrok.a", "scpi_vxi.o", "vxi_clnt.o", "vxi_xdr.o"])
        .status();
}

// ─── Link directives ─────────────────────────────────────────────────────────

fn emit_link(prefix: &Path) {
    println!("cargo:rustc-link-search=native={}", prefix.join("lib").display());
    emit_windows_static_libs();
}

fn emit_link_cross(sysroot: &Path, sr_prefix: &Path) {
    println!("cargo:rustc-link-search=native={}", sr_prefix.join("lib").display());
    println!("cargo:rustc-link-search=native={}", sysroot.join("ucrt64/lib").display());

    // Force static libunwind for gnullvm targets. llvm-mingw ships BOTH
    // libunwind.a and libunwind.dll.a in the same directory; ld prefers the
    // import lib when both are present. Pre-populate a shim dir containing
    // only libunwind.a (as a symlink) and search it first → ld picks the
    // static archive → no libunwind.dll runtime dep on the final .exe.
    if let Some(libunwind_a) = find_llvm_mingw_libunwind_a() {
        let out = PathBuf::from(env::var("OUT_DIR").unwrap());
        let shim = out.join("static-shim");
        let _ = fs::create_dir_all(&shim);
        let target = shim.join("libunwind.a");
        let _ = fs::remove_file(&target);
        #[cfg(unix)] {
            let _ = std::os::unix::fs::symlink(&libunwind_a, &target);
        }
        #[cfg(not(unix))] {
            let _ = fs::copy(&libunwind_a, &target);
        }
        if target.exists() {
            println!("cargo:rustc-link-search=native={}", shim.display());
            common::log(&format!("    static-shim: {} → {}", target.display(), libunwind_a.display()));
        }
    }

    emit_windows_static_libs();
}

fn find_llvm_mingw_libunwind_a() -> Option<PathBuf> {
    for p in &[
        "/opt/llvm-mingw-ucrt/x86_64-w64-mingw32/lib/libunwind.a",
        "/opt/llvm-mingw/x86_64-w64-mingw32/lib/libunwind.a",
        "/opt/llvm-mingw/llvm-mingw-ucrt/x86_64-w64-mingw32/lib/libunwind.a",
    ] {
        let path = std::path::Path::new(p);
        if path.is_file() { return Some(path.to_path_buf()); }
    }
    None
}

fn emit_windows_static_libs() {
    // Static: libsigrok + glib family + transitive deps.
    // Order matters (left-to-right — higher-level libs before their deps).
    for lib in [
        "sigrok",
        "gio-2.0", "gobject-2.0", "gmodule-2.0", "glib-2.0",
        "ffi", "pcre2-8",
        "intl", "iconv", "charset",
        "zip", "bz2", "lzma", "zstd",
        "usb-1.0",
        "z",
    ] {
        println!("cargo:rustc-link-lib=static={lib}");
    }

    // Win32 API surface.
    for lib in [
        "ws2_32", "ole32", "oleaut32", "shell32", "advapi32",
        "uuid", "setupapi", "iphlpapi", "winmm", "version",
        "userenv", "psapi", "dbghelp", "crypt32", "imm32",
        "bcrypt", "ntdll",
    ] {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
}
