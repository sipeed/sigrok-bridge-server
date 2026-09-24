//! Shared bootstrap helpers — filesystem utilities, process runners, cache
//! directory resolution, github mirror rewriting. Used by both the Linux
//! (`build/linux.rs`) and Windows (`build/windows.rs`) target-specific
//! bootstrap paths.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ─── Pinned upstream versions (shared by linux + windows targets) ───────────

/// Default libsigrok commit (sipeed/libsigrok @ slogic-dev, snapshot).
/// Override at build time with `LIBSIGROK_COMMIT=<sha>` to track a newer
/// upstream; the bootstrap fetches the matching GitHub archive tarball.
pub const LIBSIGROK_COMMIT_DEFAULT: &str = "6027cbfffba619a0e681928466c907f3e356ea20";
pub const LIBSIGROK_TARBALL_URL_TPL: &str =
    "https://github.com/sipeed/libsigrok/archive/{COMMIT}.tar.gz";

pub fn libsigrok_commit() -> String {
    env::var("LIBSIGROK_COMMIT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| LIBSIGROK_COMMIT_DEFAULT.to_string())
}

pub fn libsigrok_tarball_url(commit: &str) -> String {
    LIBSIGROK_TARBALL_URL_TPL.replace("{COMMIT}", commit)
}

// ─── Local libsigrok source (unified source of truth) ───────────────────────

/// The repo's own `sources/libsigrok` submodule, so the bridge links the exact
/// libsigrok the rest of the tree develops against instead of a pinned upstream
/// tarball. Resolution:
///   1. `$LIBSIGROK_SRC_DIR`               (the AIO build sets this to the mount)
///   2. `<manifest>/../../libsigrok`        (superproject sibling layout)
/// Returns `None` when neither exists, so a standalone `cargo build` outside the
/// superproject still falls back to the pinned tarball download.
pub fn libsigrok_src_dir() -> Option<PathBuf> {
    if let Ok(d) = env::var("LIBSIGROK_SRC_DIR") {
        let d = d.trim();
        if !d.is_empty() {
            let p = PathBuf::from(d);
            if is_libsigrok_tree(&p) {
                return Some(p);
            }
            panic!(
                "\nLIBSIGROK_SRC_DIR={} is not a libsigrok source tree \
                 (no autogen.sh / configure.ac).\n",
                p.display()
            );
        }
    }
    let sibling = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../libsigrok");
    if is_libsigrok_tree(&sibling) {
        return Some(sibling.canonicalize().unwrap_or(sibling));
    }
    None
}

fn is_libsigrok_tree(p: &Path) -> bool {
    p.join("autogen.sh").is_file() || p.join("configure.ac").is_file()
}

/// Identity of the local libsigrok source, used in the build stamp so a changed
/// source rebuilds libsigrok. Resolution:
///   1. `$LIBSIGROK_SRC_REV`               (the AIO computes the git rev on the host)
///   2. `git -C <dir> rev-parse HEAD` (+ `-dirty`)   (standalone with a checkout)
///   3. `None`                             (no identity — caller always rebuilds)
pub fn libsigrok_src_rev(dir: &Path) -> Option<String> {
    if let Ok(r) = env::var("LIBSIGROK_SRC_REV") {
        let r = r.trim();
        if !r.is_empty() {
            return Some(r.to_string());
        }
    }
    let head = Command::new("git")
        .args(["-C"]).arg(dir).args(["rev-parse", "HEAD"])
        .output().ok().filter(|o| o.status.success())?;
    let mut rev = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if rev.is_empty() {
        return None;
    }
    let dirty = Command::new("git")
        .args(["-C"]).arg(dir).args(["status", "--porcelain"])
        .output().ok();
    if let Some(o) = dirty {
        if o.status.success() && !o.stdout.is_empty() {
            rev.push_str("-dirty");
        }
    }
    Some(rev)
}

/// Copy a source tree into a fresh build directory, dropping VCS and autotools
/// build cruft so `autogen.sh`/`configure` regenerate cleanly. Prefers rsync
/// (present in the AIO image); falls back to `cp -a`.
pub fn copy_tree(src: &Path, dst: &Path) {
    if dst.exists() {
        fs::remove_dir_all(dst).expect("remove stale libsigrok source copy");
    }
    fs::create_dir_all(dst).expect("create libsigrok source copy dir");
    if on_path("rsync") {
        run(
            Command::new("rsync")
                .args([
                    "-a", "--delete",
                    "--exclude=.git", "--exclude=autom4te.cache",
                    "--exclude=.libs", "--exclude=.deps",
                ])
                .arg(format!("{}/", src.display()))
                .arg(dst),
            "copy libsigrok source (rsync)",
        );
    } else {
        run(
            Command::new("cp").arg("-a")
                .arg(format!("{}/.", src.display()))
                .arg(dst),
            "copy libsigrok source (cp)",
        );
        let _ = fs::remove_dir_all(dst.join(".git"));
        let _ = fs::remove_file(dst.join(".git"));
        let _ = fs::remove_dir_all(dst.join("autom4te.cache"));
    }
}

// ─── Cache directory ─────────────────────────────────────────────────────────

/// Persistent cache root for downloaded tarballs and MSYS2 packages.
/// Survives `cargo clean`, so subsequent builds only re-run compile steps.
///
/// Resolution order:
///   1. $LIBSIGROK_SYS_CACHE_DIR      (docker script sets this to a bind mount)
///   2. $XDG_CACHE_HOME/libsigrok-sys (standard XDG for user caches)
///   3. $HOME/.cache/libsigrok-sys
///   4. $OUT_DIR/cache                (last resort — lost on cargo clean)
pub fn cache_dir() -> PathBuf {
    if let Ok(d) = env::var("LIBSIGROK_SYS_CACHE_DIR") {
        if !d.is_empty() { return PathBuf::from(d); }
    }
    // Backwards-compat with the previous tarball-only env var.
    if let Ok(d) = env::var("LIBSIGROK_SYS_TARBALL_DIR") {
        if !d.is_empty() {
            if let Some(parent) = Path::new(&d).parent() {
                return parent.to_path_buf();
            }
        }
    }
    if let Ok(d) = env::var("XDG_CACHE_HOME") {
        if !d.is_empty() { return PathBuf::from(d).join("libsigrok-sys"); }
    }
    if let Ok(h) = env::var("HOME") {
        if !h.is_empty() { return PathBuf::from(h).join(".cache/libsigrok-sys"); }
    }
    PathBuf::from(env::var("OUT_DIR").unwrap()).join("cache")
}

/// Optional github.com → mirror rewriter, for users behind restricted networks.
/// Honors `$LIBSIGROK_SYS_GITHUB_MIRROR` (e.g. `https://ghproxy.com`).
pub fn resolve_url(url: &str) -> String {
    if url.starts_with("https://github.com/") || url.starts_with("http://github.com/") {
        if let Ok(mirror) = env::var("LIBSIGROK_SYS_GITHUB_MIRROR") {
            let m = mirror.trim();
            if !m.is_empty() {
                return format!("{}/{}", m.trim_end_matches('/'), url);
            }
        }
    }
    url.to_string()
}

// ─── Tool detection ──────────────────────────────────────────────────────────

pub fn on_path(cmd: &str) -> bool {
    let Some(path) = env::var_os("PATH") else { return false; };
    for dir in env::split_paths(&path) {
        let p = dir.join(cmd);
        if !p.is_file() { continue; }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(m) = fs::metadata(&p) {
                if m.permissions().mode() & 0o111 != 0 { return true; }
            }
        }
        #[cfg(not(unix))]
        return true;
    }
    false
}

/// Assert every tool is present, panic with an actionable error listing the
/// missing ones. `required[(alternates, pretty_name)]` — a tool counts as
/// present if any of its alternate command names is on PATH.
pub fn require_tools(required: &[(&[&str], &str)], install_hints: &str) {
    let mut missing: Vec<&str> = Vec::new();
    for (alternates, pretty) in required {
        if !alternates.iter().any(|c| on_path(c)) {
            missing.push(pretty);
        }
    }
    if missing.is_empty() { return; }

    let list = missing.join("\n      • ");
    panic!(
        "\nlibsigrok-sys: host toolchain incomplete — cannot bootstrap.\n\n\
         Missing:\n      • {list}\n\n{install_hints}\n"
    );
}

// ─── Fetch + extract + git ───────────────────────────────────────────────────

pub fn fetch(url: &str, out_path: &Path) {
    if out_path.exists() {
        log(&format!("    cached: {}", out_path.file_name().unwrap().to_string_lossy()));
        return;
    }
    let resolved = resolve_url(url);
    log(&format!("    download: {resolved}"));

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).expect("create tarball parent dir");
    }

    let tmp = out_path.with_extension("tmp");
    let status = Command::new("curl")
        .args(["-fL", "--retry", "3", "--retry-delay", "2"])
        .arg(&resolved).arg("-o").arg(&tmp)
        .status()
        .unwrap_or_else(|e| panic!("spawn curl: {e}"));

    if !status.success() {
        let _ = fs::remove_file(&tmp);
        panic!(
            "\ndownload failed: {resolved}\n\n\
             Workarounds:\n\
             \x20  1. LIBSIGROK_SYS_GITHUB_MIRROR=https://ghproxy.com (China mirror)\n\
             \x20  2. drop the file manually at: {}\n",
            out_path.display()
        );
    }
    fs::rename(&tmp, out_path).expect("mv tmp → final");
}

pub fn extract(tarball: &Path, dest: &Path) {
    log(&format!("    extract: {}", tarball.file_name().unwrap().to_string_lossy()));
    fs::create_dir_all(dest).expect("create extract dest");
    let status = Command::new("tar")
        .arg("-xf").arg(tarball).arg("-C").arg(dest)
        .status()
        .unwrap_or_else(|e| panic!("spawn tar: {e}"));
    if !status.success() { panic!("tar extraction failed: {}", tarball.display()); }
}

// ─── Shell helpers ───────────────────────────────────────────────────────────

pub fn run(cmd: &mut Command, what: &str) {
    log(&format!("    $ {what}"));
    let status = cmd
        .stdout(Stdio::inherit()).stderr(Stdio::inherit())
        .status()
        .unwrap_or_else(|e| panic!("spawn ({what}): {e}"));
    if !status.success() {
        panic!("\nstep failed: {what} (exit {:?})\n", status.code());
    }
}

pub fn num_jobs() -> String {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).to_string()
}

pub fn log(msg: &str) {
    println!("cargo:warning={msg}");
}

pub fn emit_rerun() {
    println!("cargo:rerun-if-changed=patches/libsigrok-bridge-fixes.patch");
    for k in [
        "LIBSIGROK_SYS_CACHE_DIR",
        "LIBSIGROK_SYS_TARBALL_DIR",
        "LIBSIGROK_SYS_GITHUB_MIRROR",
        "LIBSIGROK_COMMIT",
        "LIBSIGROK_SRC_DIR",
        "LIBSIGROK_SRC_REV",
    ] {
        println!("cargo:rerun-if-env-changed={k}");
    }
}
