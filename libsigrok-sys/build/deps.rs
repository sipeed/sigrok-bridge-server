//! Per-library bootstrap functions, parameterized by a `Platform` enum.
//!
//! Each `build_<lib>(ctx)` is idempotent: it checks for a sentinel `.a` file
//! under `$ctx.prefix/lib/` and no-ops if present. So subsequent `cargo build`
//! invocations only re-link Rust; libraries only rebuild when `cargo clean`
//! wipes `$OUT_DIR`.
//!
//! Dep graph (build in this order):
//!
//!   libffi      zlib
//!      \         |  \
//!       \   libusb   libzip
//!        \     \       |
//!        pcre2  \      |    libiconv (Windows only)
//!           \    \     |    /
//!            \    \    |   /
//!              ─── glib2 ───
//!                   │
//!              libsigrok

use std::path::PathBuf;
use std::process::Command;

use crate::common;

// ─── Pinned upstream versions ────────────────────────────────────────────────

const LIBFFI_URL:   &str = "https://github.com/libffi/libffi/releases/download/v3.4.6/libffi-3.4.6.tar.gz";
const LIBFFI_VER:   &str = "3.4.6";
const ZLIB_URL:     &str = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz";
const ZLIB_VER:     &str = "1.3.1";
const LIBUSB_URL:   &str = "https://github.com/libusb/libusb/releases/download/v1.0.27/libusb-1.0.27.tar.bz2";
const LIBUSB_VER:   &str = "1.0.27";
const LIBZIP_URL:   &str = "https://github.com/nih-at/libzip/releases/download/v1.10.1/libzip-1.10.1.tar.gz";
const LIBZIP_VER:   &str = "1.10.1";
const PCRE2_URL:    &str = "https://github.com/PCRE2Project/pcre2/releases/download/pcre2-10.44/pcre2-10.44.tar.gz";
const PCRE2_VER:    &str = "10.44";
const LIBICONV_URL: &str = "https://ftp.gnu.org/pub/gnu/libiconv/libiconv-1.17.tar.gz";
const LIBICONV_VER: &str = "1.17";
// GNOME publishes glib releases at download.gnome.org. The URL pattern embeds
// the major.minor series as a subdirectory and the full version as filename.
const GLIB_VER:     &str = "2.80.0";
const GLIB_URL:     &str = "https://download.gnome.org/sources/glib/2.80/glib-2.80.0.tar.xz";

// ─── Platform adapter ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Linux,
    Windows,
    MacOS,
}

/// Cross-compile toolchain description. Populated by `build/windows.rs` when
/// `HOST != TARGET` (i.e. Linux host → Windows target); otherwise `None`.
/// Native builds leave `Ctx.cross = None` and autotools/cmake/meson use their
/// own auto-detection.
pub struct CrossConfig {
    pub host_triple:     String,  // autotools --host= value, e.g. x86_64-w64-mingw32
    pub cc:              String,
    pub cxx:             String,
    pub ar:              String,
    pub ranlib:          String,
    pub strip:           String,
    pub windres:         String,
    /// CMake toolchain file, pre-written into $OUT_DIR.
    pub cmake_toolchain: PathBuf,
    /// Meson cross file, pre-written into $OUT_DIR.
    pub meson_cross:     PathBuf,
}

pub struct Ctx {
    pub prefix:   PathBuf,
    pub src:      PathBuf,
    pub tarballs: PathBuf,
    pub platform: Platform,
    pub cross:    Option<CrossConfig>,
}

// ─── Cross-compile helpers ───────────────────────────────────────────────────

/// For autotools: `./configure --host=<triple>` + `CC=...` env.
fn apply_autotools_cross(cmd: &mut Command, ctx: &Ctx) {
    if let Some(x) = &ctx.cross {
        cmd.arg(format!("--host={}", x.host_triple))
           .env("CC",     &x.cc)
           .env("CXX",    &x.cxx)
           .env("AR",     &x.ar)
           .env("RANLIB", &x.ranlib)
           .env("STRIP",  &x.strip)
           .env("RC",     &x.windres);
    }
}

/// For make/make install: propagate CC/AR so recipes use cross tools.
fn apply_make_cross(cmd: &mut Command, ctx: &Ctx) {
    if let Some(x) = &ctx.cross {
        cmd.env("CC",     &x.cc)
           .env("CXX",    &x.cxx)
           .env("AR",     &x.ar)
           .env("RANLIB", &x.ranlib);
    }
}

/// For cmake: pass `-DCMAKE_TOOLCHAIN_FILE=...` on cross, else nothing.
fn apply_cmake_cross(cmd: &mut Command, ctx: &Ctx) {
    if let Some(x) = &ctx.cross {
        cmd.arg(format!("-DCMAKE_TOOLCHAIN_FILE={}", x.cmake_toolchain.display()));
    }
}

/// For meson: pass `--cross-file <path>` on cross.
fn apply_meson_cross(cmd: &mut Command, ctx: &Ctx) {
    if let Some(x) = &ctx.cross {
        cmd.arg("--cross-file").arg(&x.meson_cross);
    }
}

// ─── Aggregate builder ───────────────────────────────────────────────────────

pub fn build_all(ctx: &Ctx) {
    build_libffi(ctx);
    build_zlib(ctx);
    build_libusb(ctx);
    build_libzip(ctx);
    build_pcre2(ctx);
    if ctx.platform == Platform::Windows {
        build_libiconv(ctx);
    }
    build_glib(ctx);
    build_libsigrok(ctx);
}

// ─── Per-library builders ────────────────────────────────────────────────────

pub fn build_libffi(ctx: &Ctx) {
    if ctx.prefix.join("lib/libffi.a").exists() { return; }
    common::log(">>> [1/8] libffi");

    let tarball  = ctx.tarballs.join(format!("libffi-{LIBFFI_VER}.tar.gz"));
    let builddir = ctx.src.join(format!("libffi-{LIBFFI_VER}"));
    common::fetch(LIBFFI_URL, &tarball);
    let _ = std::fs::remove_dir_all(&builddir);
    common::extract(&tarball, &ctx.src);

    let mut cfg = Command::new("./configure");
    cfg.current_dir(&builddir)
        .arg(format!("--prefix={}", ctx.prefix.display()))
        .arg("--enable-static").arg("--disable-shared").arg("--disable-docs");
    apply_autotools_cross(&mut cfg, ctx);
    common::run(&mut cfg, "libffi: configure");

    let mut mk = Command::new("make");
    mk.current_dir(&builddir).args(["-j", &common::num_jobs()]);
    apply_make_cross(&mut mk, ctx);
    common::run(&mut mk, "libffi: make");

    common::run(Command::new("make").current_dir(&builddir).arg("install"), "libffi: install");
}

pub fn build_zlib(ctx: &Ctx) {
    if ctx.prefix.join("lib/libz.a").exists() { return; }
    common::log(">>> [2/8] zlib");

    let tarball  = ctx.tarballs.join(format!("zlib-{ZLIB_VER}.tar.gz"));
    let builddir = ctx.src.join(format!("zlib-{ZLIB_VER}"));
    common::fetch(ZLIB_URL, &tarball);
    if !builddir.exists() { common::extract(&tarball, &ctx.src); }

    // zlib's configure script is NOT autotools — it's a hand-written shell
    // script that doesn't understand --host. For cross-compile we set CC/AR
    // and let configure pick those up directly.
    let mut cfg = Command::new("./configure");
    cfg.current_dir(&builddir)
        .arg(format!("--prefix={}", ctx.prefix.display())).arg("--static");
    if let Some(x) = &ctx.cross {
        cfg.env("CC", &x.cc).env("AR", &x.ar).env("RANLIB", &x.ranlib);
    }
    common::run(&mut cfg, "zlib: configure");

    let mut mk = Command::new("make");
    mk.current_dir(&builddir).args(["-j", &common::num_jobs()]);
    apply_make_cross(&mut mk, ctx);
    common::run(&mut mk, "zlib: make");

    common::run(Command::new("make").current_dir(&builddir).arg("install"), "zlib: install");
}

pub fn build_libusb(ctx: &Ctx) {
    if ctx.prefix.join("lib/libusb-1.0.a").exists() { return; }
    common::log(">>> [3/8] libusb");

    let tarball  = ctx.tarballs.join(format!("libusb-{LIBUSB_VER}.tar.bz2"));
    let builddir = ctx.src.join(format!("libusb-{LIBUSB_VER}"));
    common::fetch(LIBUSB_URL, &tarball);
    if !builddir.exists() { common::extract(&tarball, &ctx.src); }

    let mut cfg = Command::new("./configure");
    cfg.current_dir(&builddir)
        .arg(format!("--prefix={}", ctx.prefix.display()))
        .arg("--enable-static").arg("--disable-shared");
    if ctx.platform == Platform::Linux && ctx.cross.is_none() {
        // Linux-only: drop libudev runtime dep (hotplug). We poll instead.
        cfg.arg("--disable-udev");
    }
    apply_autotools_cross(&mut cfg, ctx);
    common::run(&mut cfg, "libusb: configure");

    let mut mk = Command::new("make");
    mk.current_dir(&builddir).args(["-j", &common::num_jobs()]);
    apply_make_cross(&mut mk, ctx);
    common::run(&mut mk, "libusb: make");

    common::run(Command::new("make").current_dir(&builddir).arg("install"), "libusb: install");
}

pub fn build_libzip(ctx: &Ctx) {
    if ctx.prefix.join("lib/libzip.a").exists() { return; }
    common::log(">>> [4/8] libzip (cmake)");

    let tarball  = ctx.tarballs.join(format!("libzip-{LIBZIP_VER}.tar.gz"));
    let builddir = ctx.src.join(format!("libzip-{LIBZIP_VER}"));
    common::fetch(LIBZIP_URL, &tarball);
    if !builddir.exists() { common::extract(&tarball, &ctx.src); }

    let cm = builddir.join("build");
    let _ = std::fs::remove_dir_all(&cm);
    // Disable every compression/crypto backend — each would drop a .so/.dll/.dylib
    // dep into our exe. sipeed-slogic firmware .sr files are plain deflate.
    let mut cm = Command::new("cmake");
    cm.current_dir(&builddir)
        .args(["-S", ".", "-B", "build", "-G", "Ninja"])
        .arg(format!("-DCMAKE_INSTALL_PREFIX={}", ctx.prefix.display()))
        .arg(format!("-DCMAKE_PREFIX_PATH={}",    ctx.prefix.display()))
        .args([
            "-DCMAKE_BUILD_TYPE=Release",
            "-DBUILD_SHARED_LIBS=OFF",
            "-DENABLE_BZIP2=OFF", "-DENABLE_LZMA=OFF", "-DENABLE_ZSTD=OFF",
            "-DENABLE_OPENSSL=OFF", "-DENABLE_GNUTLS=OFF", "-DENABLE_MBEDTLS=OFF",
            "-DBUILD_TOOLS=OFF", "-DBUILD_REGRESS=OFF",
            "-DBUILD_EXAMPLES=OFF", "-DBUILD_DOC=OFF",
            "-DZLIB_USE_STATIC_LIBS=ON",
        ])
        .arg(format!("-DZLIB_ROOT={}", ctx.prefix.display()));
    apply_cmake_cross(&mut cm, ctx);
    common::run(&mut cm, "libzip: cmake configure");
    common::run(Command::new("cmake").current_dir(&builddir).args(["--build", "build", "-j", &common::num_jobs()]), "libzip: build");
    common::run(Command::new("cmake").current_dir(&builddir).args(["--install", "build"]), "libzip: install");
}

pub fn build_pcre2(ctx: &Ctx) {
    if ctx.prefix.join("lib/libpcre2-8.a").exists() { return; }
    common::log(">>> [5/8] pcre2 (glib dep)");

    let tarball  = ctx.tarballs.join(format!("pcre2-{PCRE2_VER}.tar.gz"));
    let builddir = ctx.src.join(format!("pcre2-{PCRE2_VER}"));
    common::fetch(PCRE2_URL, &tarball);
    if !builddir.exists() { common::extract(&tarball, &ctx.src); }

    let mut cfg = Command::new("./configure");
    cfg.current_dir(&builddir)
        .arg(format!("--prefix={}", ctx.prefix.display()))
        .arg("--enable-static").arg("--disable-shared")
        .arg("--enable-pcre2-8")
        // pcre2 embeds its own build machinery for tests; skip them.
        .arg("--disable-pcre2test").arg("--disable-pcre2grep");
    apply_autotools_cross(&mut cfg, ctx);
    common::run(&mut cfg, "pcre2: configure");

    let mut mk = Command::new("make");
    mk.current_dir(&builddir).args(["-j", &common::num_jobs()]);
    apply_make_cross(&mut mk, ctx);
    common::run(&mut mk, "pcre2: make");

    common::run(Command::new("make").current_dir(&builddir).arg("install"), "pcre2: install");
}

pub fn build_libiconv(ctx: &Ctx) {
    // Windows only: UCRT has no iconv. Linux glibc and macOS Darwin libc
    // provide their own iconv — no bundle needed there.
    if ctx.prefix.join("lib/libiconv.a").exists() { return; }
    common::log(">>> [6/8] libiconv (Windows only — UCRT has no iconv)");

    let tarball  = ctx.tarballs.join(format!("libiconv-{LIBICONV_VER}.tar.gz"));
    let builddir = ctx.src.join(format!("libiconv-{LIBICONV_VER}"));
    common::fetch(LIBICONV_URL, &tarball);
    if !builddir.exists() { common::extract(&tarball, &ctx.src); }

    let mut cfg = Command::new("./configure");
    cfg.current_dir(&builddir)
        .arg(format!("--prefix={}", ctx.prefix.display()))
        .arg("--enable-static").arg("--disable-shared")
        .arg("--disable-nls");
    apply_autotools_cross(&mut cfg, ctx);
    common::run(&mut cfg, "libiconv: configure");

    let mut mk = Command::new("make");
    mk.current_dir(&builddir).args(["-j", &common::num_jobs()]);
    apply_make_cross(&mut mk, ctx);
    common::run(&mut mk, "libiconv: make");

    common::run(Command::new("make").current_dir(&builddir).arg("install"), "libiconv: install");
}

pub fn build_glib(ctx: &Ctx) {
    // Check for glib-2.0 sentinel. The install lays down libglib-2.0.a +
    // libgio-2.0.a + libgobject-2.0.a + libgmodule-2.0.a in one shot.
    if ctx.prefix.join("lib/libglib-2.0.a").exists() { return; }
    common::log(&format!(">>> [7/8] glib {GLIB_VER}"));

    let tarball  = ctx.tarballs.join(format!("glib-{GLIB_VER}.tar.xz"));
    let builddir = ctx.src.join(format!("glib-{GLIB_VER}"));
    common::fetch(GLIB_URL, &tarball);
    if !builddir.exists() { common::extract(&tarball, &ctx.src); }

    let build = builddir.join("build");
    let _ = std::fs::remove_dir_all(&build);

    // Meson options. Goal: build just libglib + libgio + libgobject + libgmodule
    // statically, drop everything else.
    //   default_library=static : produce .a only, no .so/.dll
    //   tests=false            : don't build the test suite
    //   nls=disabled           : no libintl / libcharset deps
    //   libmount/selinux/      : disable everything that pulls extra .so
    //     sysprof/xattr/libelf : on Linux
    //   introspection=disabled : no gobject-introspection build machinery
    //   man-pages/docs         : skip
    // iconv: auto-detected from platform (glibc libc, Darwin libc, MSYS2
    // libiconv via pkg-config). glib 2.80 doesn't expose an explicit option.
    let mut meson = Command::new("meson");
    meson.current_dir(&builddir)
        .env("PKG_CONFIG_PATH",   ctx.prefix.join("lib/pkgconfig"))
        .env("PKG_CONFIG_LIBDIR", ctx.prefix.join("lib/pkgconfig"))
        .args(["setup", "build"])
        .arg(format!("--prefix={}", ctx.prefix.display()))
        .args([
            "--buildtype=release",
            "--default-library=static",
            "-Dtests=false",
            "-Dinstalled_tests=false",
            "-Dnls=disabled",
            "-Dglib_debug=disabled",
            "-Dintrospection=disabled",
            "-Dman-pages=disabled",
            "-Ddocumentation=false",
            "-Dsysprof=disabled",
            "-Dselinux=disabled",
            "-Dlibmount=disabled",
            "-Dxattr=false",
        ]);
    apply_meson_cross(&mut meson, ctx);
    common::run(&mut meson, "glib: meson setup");
    common::run(
        Command::new("meson").current_dir(&builddir).args(["compile", "-C", "build"]),
        "glib: meson compile");
    common::run(
        Command::new("meson").current_dir(&builddir).args(["install", "-C", "build"]),
        "glib: meson install");

    assert!(ctx.prefix.join("lib/libglib-2.0.a").exists(),
            "glib install didn't produce libglib-2.0.a");
}

pub fn build_libsigrok(ctx: &Ctx) {
    if ctx.prefix.join("lib/libsigrok.a").exists() { return; }
    common::log(">>> [8/8] libsigrok");

    let commit = common::libsigrok_commit();
    common::log(&format!("    commit: {commit}"));

    let tarball  = ctx.tarballs.join(format!("libsigrok-{commit}.tar.gz"));
    let builddir = ctx.src.join(format!("libsigrok-{commit}"));
    common::fetch(&common::libsigrok_tarball_url(&commit), &tarball);
    if !builddir.exists() { common::extract(&tarball, &ctx.src); }

    // VXI-11 patch: drop the scpi_vxi_dev registration. scpi_vxi.o would
    // otherwise force libtirpc + the krb5 chain via xdr_*.
    common::run(
        Command::new("sed").current_dir(&builddir)
            .args(["-i", "/scpi_vxi_dev/d", "src/scpi/scpi.c"]),
        "libsigrok: patch out VXI-11");

    common::run(Command::new("./autogen.sh").current_dir(&builddir), "libsigrok: autogen");

    // -ffunction-sections -fdata-sections so final linker's --gc-sections can
    // drop unused functions at sub-.o granularity. Windows static also needs
    // GLib's static-compilation macros so headers don't mark symbols as dllimport.
    let mut cflags = String::from("-ffunction-sections -fdata-sections -O2");
    if ctx.platform == Platform::Windows {
        cflags.push_str(
            " -DGLIB_STATIC_COMPILATION -DGIO_STATIC_COMPILATION \
              -DGOBJECT_STATIC_COMPILATION -DGMODULE_STATIC_COMPILATION \
              -DPCRE2_STATIC");
    }

    let pkg_path = ctx.prefix.join("lib/pkgconfig");
    let mut cfg = Command::new("./configure");
    cfg.current_dir(&builddir)
        .env("PKG_CONFIG_PATH",   &pkg_path)
        .env("PKG_CONFIG_LIBDIR", &pkg_path)
        .env("CFLAGS",  &cflags)
        .arg(format!("--prefix={}", ctx.prefix.display()))
        .arg("--disable-all-drivers").arg("--enable-sipeed-slogic-analyzer")
        .arg("--disable-bindings")
        .arg("--enable-static").arg("--disable-shared")
        .arg("--without-libserialport").arg("--without-libftdi")
        .arg("--without-libhidapi").arg("--without-libbluez")
        .arg("--without-libnettle")
        .arg("--without-librevisa").arg("--without-libgpib").arg("--without-libieee1284");
    apply_autotools_cross(&mut cfg, ctx);
    common::run(&mut cfg, "libsigrok: configure");

    let mut mk = Command::new("make");
    mk.current_dir(&builddir).args(["-j", &common::num_jobs()]);
    apply_make_cross(&mut mk, ctx);
    common::run(&mut mk, "libsigrok: make");

    common::run(Command::new("make").current_dir(&builddir).arg("install"), "libsigrok: install");

    // Post-install surgery: strip VXI transport objects. Use cross-ar when
    // available (archives from mingw toolchain); native `ar` handles the rest.
    let ar_tool = ctx.cross.as_ref().map(|x| x.ar.as_str()).unwrap_or("ar");
    let _ = Command::new(ar_tool)
        .current_dir(ctx.prefix.join("lib"))
        .args(["d", "libsigrok.a", "scpi_vxi.o", "vxi_clnt.o", "vxi_xdr.o"])
        .status();

    // libsigrok.pc still lists stale transport deps in Requires.private and
    // -ltirpc in Libs.private. Strip so `pkg-config --static --libs libsigrok`
    // emits a clean link line.
    let _ = Command::new("sed")
        .current_dir(ctx.prefix.join("lib/pkgconfig"))
        .args([
            "-i",
            "-e", "s/^Requires.private:.*/Requires.private: zlib libusb-1.0 gio-2.0 libzip/",
            "-e", "s/-ltirpc //g",
            "-e", "s/ -ltirpc//g",
            "libsigrok.pc",
        ])
        .status();
}
