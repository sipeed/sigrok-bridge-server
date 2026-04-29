//! MSYS2 ucrt64 package bootstrap for Linux → Windows cross-compile.
//!
//! Instead of re-building every C dependency from source against a cross
//! toolchain (slow + fragile), we fetch the pre-built `.pkg.tar.zst` files
//! that MSYS2 already publishes. They ship both `.dll.a` (import libs) and
//! `.a` (static archives), so we link statically against the `.a` files —
//! just like a native MSYS2 build would.
//!
//! Flow:
//!   1. `pacman -Syw --cachedir <cache/msys2>` — resolve transitive deps,
//!      download packages into the persistent cache. (The caller provides
//!      a `pacman` binary; Arch has it natively, Debian ships
//!      `pacman-package-manager`.)
//!   2. Extract each cached `.pkg.tar.zst` into `$OUT_DIR/sysroot/` via
//!      plain `tar --zstd -xf`. Creates the usual MSYS2 layout
//!      (`sysroot/ucrt64/{bin,include,lib,lib/pkgconfig}`).
//!   3. Record resolved filenames to `manifest.txt` — subsequent runs skip
//!      pacman entirely and re-extract from cache for reproducibility.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::common;

// Top-level packages — UCRT ucrt64 variant, matching llvm-mingw's CRT.
// pacman walks the ucrt64 DB and pulls transitive deps:
//   gcc-libs → libwinpthread, libgcc
//   glib2    → libffi, pcre2, libiconv, gettext (libintl), zlib
//   libzip   → bzip2, xz, zstd
const TOP_LEVEL: &[&str] = &[
    "mingw-w64-ucrt-x86_64-gcc-libs",
    "mingw-w64-ucrt-x86_64-glib2",
    "mingw-w64-ucrt-x86_64-libusb",
    "mingw-w64-ucrt-x86_64-libzip",
];

const MIRRORS: &[&str] = &[
    "https://mirror.msys2.org/mingw/ucrt64/",
    "https://repo.msys2.org/mingw/ucrt64/",
];

/// Download + extract MSYS2 ucrt64 packages into `$sysroot/ucrt64/*`.
/// Idempotent: a sentinel (`ucrt64/lib/pkgconfig/glib-2.0.pc`) short-circuits.
pub fn bootstrap(sysroot: &Path, cache_root: &Path) {
    let sentinel = sysroot.join("ucrt64/lib/pkgconfig/glib-2.0.pc");
    if sentinel.exists() {
        common::log(&format!(">>> msys2 sysroot cached: {}", sysroot.display()));
        return;
    }
    common::log(">>> msys2 sysroot: bootstrapping");

    let pkg_cache = cache_root.join("msys2");
    fs::create_dir_all(&pkg_cache).expect("mkdir msys2 cache");
    let manifest = pkg_cache.join("manifest.txt");

    // Fast path: manifest from a previous run pins exact package filenames.
    // If every listed .pkg.tar.zst is still in cache, skip pacman entirely —
    // two cold builds of the same commit get byte-identical sysroots.
    if let Some(files) = valid_manifest(&manifest, &pkg_cache) {
        extract_many(&files, &pkg_cache, sysroot);
        verify(sysroot);
        return;
    }

    // Cold path: pacman resolves deps + downloads to pkg_cache.
    // fakeroot bypasses pacman's hard-coded getuid()==0 check.
    for tool in ["pacman", "fakeroot"] {
        if !common::on_path(tool) {
            panic!(
                "\nLinux → Windows cross requires `{tool}` to resolve MSYS2 packages.\n\
                 Arch:    sudo pacman -S pacman fakeroot (fakeroot is in base-devel)\n\
                 Debian:  sudo apt install pacman-package-manager fakeroot\n\
                 Fedora:  sudo dnf install pacman fakeroot\n\n\
                 Or pre-populate {cache} with the MSYS2 .pkg.tar.zst files + a manifest.txt\n\
                 listing their filenames, one per line.\n",
                cache = pkg_cache.display()
            );
        }
    }

    let pac_db = pkg_cache.join(".pacman-db");
    let pac_conf = write_pacman_conf(&pac_db);
    resolve_and_download(&pac_conf, &pac_db, &pkg_cache);

    let resolved = query_resolved(&pac_conf, &pac_db);
    if resolved.is_empty() {
        panic!("pacman resolved empty package set — bad repo config?");
    }

    extract_many(&resolved, &pkg_cache, sysroot);
    verify(sysroot);

    fs::write(&manifest, resolved.join("\n") + "\n").expect("write manifest");
    common::log(&format!(">>> msys2 manifest: {} ({} packages)",
                         manifest.display(), resolved.len()));
}

// ─── pacman setup ────────────────────────────────────────────────────────────

fn write_pacman_conf(pac_db: &Path) -> PathBuf {
    fs::create_dir_all(pac_db.join("var/lib/pacman")).expect("mkdir pacman db");
    fs::create_dir_all(pac_db.join("var/log")).expect("mkdir pacman log");
    fs::create_dir_all(pac_db.join("etc/pacman.d/gnupg")).expect("mkdir pacman gpg");
    fs::create_dir_all(pac_db.join("etc/pacman.d/hooks")).expect("mkdir pacman hooks");

    // Inline pacman.conf: SigLevel = Never because MSYS2's keyring isn't
    // accessible from a host pacman install — TLS to msys2.org is our
    // trust anchor.
    let mut conf = String::from(
        "[options]\n\
         HoldPkg = pacman\n\
         Architecture = x86_64\n\
         SigLevel = Never\n\n\
         [ucrt64]\n"
    );
    for m in MIRRORS {
        conf.push_str(&format!("Server = {m}\n"));
    }
    let path = pac_db.join("pacman.conf");
    fs::write(&path, conf).expect("write pacman.conf");
    path
}

fn resolve_and_download(conf: &Path, pac_db: &Path, pkg_cache: &Path) {
    // pacman unconditionally checks `geteuid() == 0` for -Syw and errors
    // out on a regular user account. fakeroot wraps the syscall to return
    // 0 so pacman proceeds (the actual write targets are our user-writable
    // --cachedir / --dbpath, so no real privilege is needed).
    let mut cmd = Command::new("fakeroot");
    cmd.arg("pacman")
       .arg("--config").arg(conf)
       .arg("--dbpath").arg(pac_db.join("var/lib/pacman"))
       .arg("--cachedir").arg(pkg_cache)
       .arg("--logfile").arg(pac_db.join("var/log/pacman.log"))
       .arg("--gpgdir").arg(pac_db.join("etc/pacman.d/gnupg"))
       .arg("--hookdir").arg(pac_db.join("etc/pacman.d/hooks"))
       .args(["--noconfirm", "-Syw"]);
    for p in TOP_LEVEL { cmd.arg(p); }
    common::run(&mut cmd, "msys2: fakeroot pacman -Syw");
}

fn query_resolved(conf: &Path, pac_db: &Path) -> Vec<String> {
    // -Sp is read-only; don't need fakeroot here.
    let mut cmd = Command::new("pacman");
    cmd.arg("--config").arg(conf)
       .arg("--dbpath").arg(pac_db.join("var/lib/pacman"))
       .arg("-Sp");
    for p in TOP_LEVEL { cmd.arg(p); }

    let out = cmd.output().expect("spawn pacman -Sp");
    if !out.status.success() {
        panic!("pacman -Sp failed:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.rsplit('/').next().map(str::to_string))
        .filter(|s| s.ends_with(".pkg.tar.zst") || s.ends_with(".pkg.tar.xz"))
        .collect()
}

// ─── Manifest + extraction ───────────────────────────────────────────────────

fn valid_manifest(manifest: &Path, pkg_cache: &Path) -> Option<Vec<String>> {
    let text = fs::read_to_string(manifest).ok()?;
    let files: Vec<String> = text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    if files.is_empty() { return None; }
    if files.iter().all(|f| pkg_cache.join(f).exists()) {
        common::log(&format!(">>> msys2: manifest hit ({} packages)", files.len()));
        Some(files)
    } else {
        common::log(">>> msys2: manifest stale — re-resolving");
        None
    }
}

fn extract_many(files: &[String], pkg_cache: &Path, sysroot: &Path) {
    fs::create_dir_all(sysroot).expect("mkdir sysroot");
    for f in files {
        let pkg = pkg_cache.join(f);
        if !pkg.exists() {
            panic!("manifest references {f} but it's missing from {}", pkg_cache.display());
        }
        common::log(&format!("    extract: {f}"));
        let status = Command::new("tar")
            .args(["--zstd", "-xf"])
            .arg(&pkg).arg("-C").arg(sysroot)
            .status()
            .unwrap_or_else(|e| panic!("spawn tar: {e}"));
        if !status.success() {
            // Fall back to auto-detection for the occasional .xz member.
            let status2 = Command::new("tar")
                .arg("-xf").arg(&pkg).arg("-C").arg(sysroot)
                .status()
                .unwrap_or_else(|e| panic!("spawn tar: {e}"));
            if !status2.success() { panic!("extract failed: {f}"); }
        }
    }
    // pacman metadata dropped at tar root — clean up.
    for meta in [".BUILDINFO", ".MTREE", ".INSTALL", ".PKGINFO"] {
        let _ = fs::remove_file(sysroot.join(meta));
    }
}

fn verify(sysroot: &Path) {
    let pc = sysroot.join("ucrt64/lib/pkgconfig/glib-2.0.pc");
    if !pc.exists() {
        panic!("msys2 sysroot incomplete — missing {}", pc.display());
    }
}
