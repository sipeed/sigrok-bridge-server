# sigrok-bridge-server

A Rust bridge that connects [libsigrok](https://sigrok.org/wiki/Libsigrok)-compatible
hardware to [ngscopeclient](https://www.ngscopeclient.org/) via the
`SigrokOscilloscope` driver in scopehal, using the `twinlan` transport
(text SCPI command socket + binary waveform data socket).

This project specifically adds support for **Sipeed SLogic** and its derivatives
through the upstream `sipeed-slogic-analyzer` libsigrok driver.

> **Note:** End-to-end support requires both halves of the pipeline:
>
> 1. **This Rust bridge server** (the contents of this directory)
> 2. **The C++ `SigrokOscilloscope` driver** in scopehal — shipped as a patch
>    against `scopehal-apps/lib/scopehal` under [`patches/`](patches/).
>
> See [`patches/README.md`](patches/README.md) for how to apply, refresh, and
> rebase the patch.

## Supported Hardware

**Sipeed SLogic Combo** and derivatives — libsigrok is built with
`--disable-all-drivers --enable-sipeed-slogic-analyzer`, so only the Sipeed
family is compiled in. (To enable other drivers, edit the configure flags in
`libsigrok-sys/build/deps.rs`.)

## Workspace Layout

```
sigrok-bridge-server/
├── libsigrok-sys/              # -sys crate: FFI + native bootstrap
│   ├── build.rs                #   target dispatcher (linux / windows / macos)
│   └── build/
│       ├── common.rs           #   cache, fetch, extract, tool check
│       ├── deps.rs             #   per-library build fns, shared by all platforms
│       ├── linux.rs            #   Linux platform entry
│       ├── windows.rs          #   Windows MSYS2 UCRT64 platform entry
│       └── macos.rs            #   macOS platform entry
├── sigrok-bridge-common/       # SCPI parser, twinlan transport, wire protocol
├── sigrok-bridge/              # Production bridge
├── fake/                       # Fake bridge (no hardware)
├── patches/                    # scopehal C++ patch for the client side
└── docs/
    ├── ARCHITECTURE.md
    ├── BUILD.md                # Deep dive: link order, deps, post-install surgery
    ├── DEVELOPMENT.md
    └── SCPI-PROTOCOL.md
```

`libsigrok-sys` is the single source of truth for native linkage
(`links = "sigrok"` in Cargo.toml). On any supported host, `cargo build`
downloads and compiles libffi, zlib, libusb, libzip, pcre2, libiconv (Windows
only), glib2, and libsigrok from source into `$OUT_DIR/prefix/`, then
statically links them. The resulting binary depends only on the host OS
baseline (glibc on Linux, UCRT + Win32 on Windows, libSystem + frameworks
on macOS).

## Build matrix

`cargo build --release -p sigrok-bridge` on any of the supported hosts, or
cross-compile from Linux to Windows.

| Host / Target                     | How                                                                              | Output                                                   |
|-----------------------------------|----------------------------------------------------------------------------------|----------------------------------------------------------|
| Linux host → Linux                | `cargo build --release -p sigrok-bridge`                                         | `target/release/sigrok-bridge` — ~5 MB, glibc-only       |
| Windows host → Windows (MSYS2)    | inside UCRT64 shell: `cargo build --release -p sigrok-bridge`                    | `target/release/sigrok-bridge.exe` — Win32 + UCRT only   |
| macOS host → macOS                | `cargo build --release -p sigrok-bridge`                                         | `target/release/sigrok-bridge` — libSystem + frameworks  |
| Linux host → Windows (cross)      | `cargo build --release --target x86_64-pc-windows-gnullvm -p sigrok-bridge`      | `target/x86_64-pc-windows-gnullvm/release/sigrok-bridge.exe` — ~7 MB, Win32 + UCRT only |

Native builds compile every C dep (libffi / zlib / libusb / libzip / pcre2 /
glib2 / libsigrok) from source — fully self-contained binaries.

Cross-build uses MSYS2 ucrt64 prebuilt packages (downloaded via pacman) +
compiles only libsigrok from source. Faster than full source cross, produces
the same UCRT-based `.exe`.

First build takes ~5–10 min. Subsequent builds are seconds; compiled `.a`
files cache in `$OUT_DIR`, source tarballs + MSYS2 packages live at
`~/.cache/libsigrok-sys/` (survives `cargo clean`).

## Build — Linux

```bash
# One-time prereqs (pick one)
sudo pacman -S base-devel cmake ninja meson curl tar xz bzip2 \
                autoconf automake libtool pkgconf                        # Arch
sudo apt install build-essential cmake ninja-build meson curl tar \
                xz-utils bzip2 autoconf automake libtool pkg-config      # Debian/Ubuntu
sudo dnf groupinstall 'Development Tools'
sudo dnf install cmake ninja-build meson curl tar xz bzip2 \
                autoconf automake libtool pkgconf-pkg-config              # Fedora

cargo build --release -p sigrok-bridge
```

Verify static linking:

```console
$ ldd target/release/sigrok-bridge
        linux-vdso.so.1
        libm.so.6, libgcc_s.so.1, libc.so.6, ld-linux-x86-64.so.2
```

## Build — Windows (native, via MSYS2)

1. Install [MSYS2](https://www.msys2.org/) (official installer).
2. Open the **UCRT64** shell (`mingw-w64-ucrt-x86_64` environment).
3. Install toolchain:

   ```bash
   pacman -S --needed \
       mingw-w64-ucrt-x86_64-gcc \
       mingw-w64-ucrt-x86_64-rust \
       mingw-w64-ucrt-x86_64-cmake \
       mingw-w64-ucrt-x86_64-ninja \
       mingw-w64-ucrt-x86_64-meson \
       mingw-w64-ucrt-x86_64-pkgconf \
       mingw-w64-ucrt-x86_64-python \
       autoconf automake libtool make \
       curl tar xz bzip2 sed git patch
   ```

4. Build:

   ```bash
   cd path/to/sigrok-bridge-server
   cargo build --release -p sigrok-bridge
   ```

Output: `target\release\sigrok-bridge.exe`. Verify no non-system DLL deps via
`objdump -p sigrok-bridge.exe | grep 'DLL Name'`.

## Build — Windows (cross from Linux)

Linux host compiling a Windows `.exe` via llvm-mingw + MSYS2 packages. Uses
`x86_64-pc-windows-gnullvm` (UCRT).

1. Install toolchain (Arch example):

   ```bash
   rustup target add x86_64-pc-windows-gnullvm
   sudo pacman -S mingw-w64-binutils pacman fakeroot autoconf automake libtool \
                  pkgconf make curl tar sed bash
   yay -S llvm-mingw-w64-toolchain-ucrt-bin     # AUR — provides /opt/llvm-mingw-ucrt/
   # or download github.com/mstorsjo/llvm-mingw releases and add bin/ to PATH
   ```

   On Debian/Ubuntu:

   ```bash
   sudo apt install mingw-w64 pacman-package-manager fakeroot \
                    autoconf automake libtool pkg-config make curl tar sed bash
   # llvm-mingw: download a release from github.com/mstorsjo/llvm-mingw
   ```

2. Build:

   ```bash
   cargo build --release --target x86_64-pc-windows-gnullvm -p sigrok-bridge
   ```

How it works:

- `build/msys2.rs` runs `fakeroot pacman -Syw` to download MSYS2 ucrt64
  `.pkg.tar.zst` files (glib2, libusb, libzip + transitive deps) into
  `~/.cache/libsigrok-sys/msys2/`, then extracts them to a sysroot under
  `$OUT_DIR/sysroot/`.
- libsigrok is fetched from sipeed/libsigrok @ pinned commit and cross-
  compiled against the sysroot with llvm-mingw's clang.
- libtool's partial-link step (`ld -r` for `libdrivers.o`) uses Arch's
  GNU `ld.bfd` via the `LD` env var — llvm-mingw's ld.lld doesn't accept
  `-r` in PE/COFF mode.
- A `static-shim/` dir with just `libunwind.a` (symlink) is prepended to
  rustc's link search so the final `.exe` has no `libunwind.dll` dep.

Output: `target/x86_64-pc-windows-gnullvm/release/sigrok-bridge.exe` (~7 MB).
Only Win32 API + UCRT `api-ms-win-crt-*.dll` as runtime deps.

## Build — macOS

1. Xcode command-line tools: `xcode-select --install`
2. Install build deps via [Homebrew](https://brew.sh/):

   ```bash
   brew install autoconf automake libtool pkg-config cmake ninja meson \
                curl xz bzip2
   ```

3. Build:

   ```bash
   cargo build --release -p sigrok-bridge
   ```

Output: `target/release/sigrok-bridge` (Mach-O). Verify with
`otool -L target/release/sigrok-bridge` — should show only `libSystem.B.dylib`
+ `IOKit.framework` + `CoreFoundation.framework`.

## Environment overrides (optional)

- `LIBSIGROK_COMMIT` — pin libsigrok to a different upstream SHA (default:
  tracked in `build/common.rs::LIBSIGROK_COMMIT_DEFAULT`)
- `LIBSIGROK_SYS_CACHE_DIR` — tarball cache root (default:
  `~/.cache/libsigrok-sys/`)
- `LIBSIGROK_SYS_GITHUB_MIRROR=https://ghproxy.com` — route GitHub URLs
  through a mirror (for networks with restricted github.com access)

## Run

### Fake bridge (no hardware, pure Rust)

```bash
cargo run -p fake-bridge
```

### Production bridge

```bash
./target/release/sigrok-bridge                              # port 10101
./target/release/sigrok-bridge --pattern-mode Emulation     # test pattern
```

### CLI options

| Flag                       | Default                  | Description                                                  |
|----------------------------|--------------------------|--------------------------------------------------------------|
| `-d`, `--driver <NAME>`    | `sipeed-slogic-analyzer` | libsigrok driver name                                        |
| `-p`, `--port <PORT>`      | `10101`                  | TCP port for twinlan transport (data port = port + 1)        |
| `--pattern-mode <MODE>`    | (unset)                  | Built-in test patterns: `Normal`, `"USB connection test"`, `Emulation` |

## Connect from ngscopeclient

In the *Add Instrument* dialog:

- **Driver**: `sigrok`
- **Transport**: `twinlan`
- **Path**: `localhost:10101`

The bridge accepts a single client at a time. On disconnect, the device is
released and the next client can connect.

## Architecture (short)

```
ngscopeclient (C++)              sigrok-bridge (Rust)            libsigrok (C)
─────────────────              ──────────────────────            ─────────────
SigrokOscilloscope  ──SCPI──▶  scpi_handler  ──▶  SigrokDevice  ──▶  driver
                    ◀─binary── data thread  ◀──  acquire()     ◀──  hardware
```

Full details in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md); wire protocol
in [`docs/SCPI-PROTOCOL.md`](docs/SCPI-PROTOCOL.md).

## Status of SLogic Support

- [x] Device discovery via `sipeed-slogic-analyzer` driver
- [x] Sample rate enumeration (handles `a{sv}` GVariant dict from the driver)
- [x] Sample depth fallback (1 KS to 1 TS, 1/2/5 step)
- [x] Acquisition state machine (start / single / stop / force / armed)
- [x] Continuous and one-shot capture
- [x] `SR_CONF_PATTERN_MODE` switching (Normal / USB connection test / Emulation)
- [x] Digital channel data path (16-bit unit size)
- [x] Graceful client disconnect (data thread shutdown flag)
- [x] Cross-buffer edge trigger detection (software fallback keeps last state)
