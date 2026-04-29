//! libsigrok-sys build script.
//!
//! Unconditionally bootstraps libsigrok + its static dependencies from source,
//! producing a self-contained `cargo build` output. No features, no env-var
//! switches — `cargo build -p sigrok-bridge` on any supported target always
//! ends with the libsigrok family statically linked into the final binary.
//!
//! Target dispatch:
//!   * `*-linux-*`           → `build/linux.rs` (libffi + zlib + libusb +
//!                              libzip + libsigrok from source; system glib2).
//!   * `*-windows-gnullvm`   → `build/windows.rs` (MSYS2 ucrt64 sysroot via
//!                              pacman + libsigrok cross-compile). Intended
//!                              to run inside the Dockerfile.windows image.
//!   * everything else       → explicit panic (macOS is not yet ported).
//!
//! Caching: downloaded tarballs / MSYS2 packages live under
//! `$LIBSIGROK_SYS_CACHE_DIR` (default `~/.cache/libsigrok-sys/`), so they
//! survive `cargo clean`. Override `LIBSIGROK_COMMIT` to track a different
//! upstream libsigrok SHA.

#[path = "build/common.rs"]
mod common;

#[path = "build/deps.rs"]
mod deps;

#[path = "build/msys2.rs"]
mod msys2;

#[path = "build/linux.rs"]
mod linux;

#[path = "build/windows.rs"]
mod windows;

#[path = "build/macos.rs"]
mod macos;

fn main() {
    common::emit_rerun();

    let target = std::env::var("TARGET").expect("TARGET env var");

    if target.contains("linux") {
        linux::build();
    } else if target.contains("windows") {
        windows::build();
    } else if target.contains("apple") {
        macos::build();
    } else {
        panic!("\nlibsigrok-sys: unsupported target `{target}`.\n");
    }
}
