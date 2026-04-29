# Build — deep dive

For basic usage see [`README.md`](../README.md). This doc covers internals:
dep graph, link-order rationale, post-install surgery, cache layout,
troubleshooting.

## Architecture

`libsigrok-sys/build.rs` dispatches on the Cargo `TARGET` triple, then
detects native vs cross via `HOST != TARGET`:

| Target             | Mode            | Entry                | Strategy                                                    |
|--------------------|-----------------|----------------------|-------------------------------------------------------------|
| `*-linux-*`        | native          | `build/linux.rs`     | gcc, autotools, meson — full source bundle.                 |
| `*-windows-*`      | native (MSYS2)  | `build/windows.rs`   | mingw-w64 gcc (inside UCRT64 shell) — full source bundle.   |
| `*-windows-*`      | cross (Linux)   | `build/windows.rs`   | MSYS2 ucrt64 prebuilt packages + libsigrok from source.     |
| `*-apple-darwin`   | native          | `build/macos.rs`     | Xcode clang + Homebrew autotools — full source bundle.      |

Native builds:
1. `check_toolchain()` — fail fast if prereqs missing, with actionable hints.
2. `deps::build_all(ctx)` — compile every C dep from source into
   `$OUT_DIR/prefix/`.
3. `emit_link_directives(prefix)` — tell rustc which libs to link.

Cross from Linux → Windows is different:
1. `msys2::bootstrap(sysroot, cache)` — `pacman -Syw` downloads MSYS2 ucrt64
   `.pkg.tar.zst` to persistent cache, `tar --zstd -xf` into `$OUT_DIR/sysroot/`.
   Transitive deps resolved by pacman; manifest pinned for reproducibility.
2. `build_libsigrok_cross(sysroot, sr_prefix, cc, partial_ld)` — fetch
   libsigrok source tarball, cross-compile against the sysroot using
   llvm-mingw clang. `LD=x86_64-w64-mingw32-ld.bfd` is exported so libtool's
   partial-link step uses GNU ld (supports PE `-r`) while clang's normal
   link uses ld.lld.
3. `emit_link_cross(sysroot, sr_prefix)` — same library list as native
   Windows, plus a `static-shim/libunwind.a` symlink prepended to the
   rustc search path (bypasses llvm-mingw's `libunwind.dll.a`).

## Dep graph

Eight libraries bootstrapped, always in this order:

```
libffi      zlib
   \         |  \
    \   libusb   libzip
     \     \       |
     pcre2  \      |    libiconv (Windows only)
        \    \     |    /
         \    \    |   /
           ─── glib2 ───
                │
           libsigrok
```

Each `build_<lib>(ctx)` in `build/deps.rs` checks for a sentinel `.a`
(`$prefix/lib/lib<lib>.a`) and skips when present — so a `cargo clean` after
the first build re-runs nothing until `$OUT_DIR` is wiped.

Pinned versions (in `build/deps.rs` consts):

| Library  | Version  | Source URL                                                     |
|----------|----------|----------------------------------------------------------------|
| libffi   | 3.4.6    | github.com/libffi/libffi                                       |
| zlib     | 1.3.1    | github.com/madler/zlib                                         |
| libusb   | 1.0.27   | github.com/libusb/libusb                                       |
| libzip   | 1.10.1   | github.com/nih-at/libzip                                       |
| pcre2    | 10.44    | github.com/PCRE2Project/pcre2                                  |
| libiconv | 1.17     | ftp.gnu.org/pub/gnu/libiconv (Windows only)                    |
| glib     | 2.80.0   | download.gnome.org/sources/glib                                |
| libsigrok| (SHA pin) | github.com/sipeed/libsigrok — `LIBSIGROK_COMMIT_DEFAULT`      |

## Cache layout

Persistent cache (survives `cargo clean`) at:

1. `$LIBSIGROK_SYS_CACHE_DIR` (explicit)
2. `$XDG_CACHE_HOME/libsigrok-sys`
3. `$HOME/.cache/libsigrok-sys`
4. `$OUT_DIR/cache` (last resort)

Contents:

```
<cache>/
  tarballs/
    libffi-3.4.6.tar.gz
    zlib-1.3.1.tar.gz
    libusb-1.0.27.tar.bz2
    libzip-1.10.1.tar.gz
    pcre2-10.44.tar.gz
    libiconv-1.17.tar.gz      (Windows hosts only)
    glib-2.80.0.tar.xz
    libsigrok-<commit>.tar.gz
```

Compiled `.a` files live in `$OUT_DIR/prefix/lib/` (wiped on cargo clean).

## Link-order rationale

### Linux — driven by pkg-config

`libsigrok.pc` is post-install-patched to have:

```
Requires.private: zlib libusb-1.0 gio-2.0 libzip
Libs.private:     (no -ltirpc)
```

`pkg-config --static --libs libsigrok` then expands into every `-l<name>`
needed. For each token, `build/linux.rs` emits `static=<name>` if
`lib<name>.a` is in `$prefix/lib`, else `dylib=<name>`.

`force_dynamic` list: `c`, `m`, `pthread`, `dl`, `rt`, `util`, `resolv`,
`crypt`, `nsl`, `gcc_s` — static glibc references dynamic-loader symbols
(`_dl_x86_cpu_features` etc.) and is a known portability footgun.

### Windows — hardcoded list

`build/windows.rs` emits a fixed link order (MSYS2's `.pc` files are less
reliable under static link):

```
static: sigrok → gio-2.0 → gobject-2.0 → gmodule-2.0 → glib-2.0
      → ffi → pcre2-8 → iconv → charset
      → zip → usb-1.0 → z

dylib (Win32 API): ws2_32, ole32, oleaut32, shell32, advapi32, uuid,
      setupapi, iphlpapi, winmm, version, userenv, psapi, dbghelp,
      crypt32, imm32, bcrypt, ntdll
```

### macOS — pkg-config + frameworks

Same pkg-config pattern as Linux, plus explicit
`cargo:rustc-link-lib=framework=IOKit / CoreFoundation / Security`
for libusb's Darwin USB layer.

## libsigrok VXI-11 / libtirpc excision

libsigrok's `src/scpi/scpi.c` registers `scpi_vxi_dev` even when all drivers
are disabled. That single reference keeps `scpi_vxi.o` / `vxi_clnt.o` /
`vxi_xdr.o` in the link graph, which drags in `libtirpc` + the krb5 chain.

`deps::build_libsigrok` applies a two-step fix:

1. **Source patch** (before configure):
   `sed -i '/scpi_vxi_dev/d' src/scpi/scpi.c`
   drops the registration so `--gc-sections` can strip the transport code.
2. **Archive surgery** (after install):
   `ar d libsigrok.a scpi_vxi.o vxi_clnt.o vxi_xdr.o` — belt-and-suspenders.

The installed `libsigrok.pc` is also sed-patched to drop stale `-ltirpc`
from `Libs.private` and clean up `Requires.private`.

## libsigrok configure flags

Both source patches go in, then:

```
--prefix=$OUT_DIR/prefix
--disable-all-drivers
--enable-sipeed-slogic-analyzer
--disable-bindings
--enable-static --disable-shared
--without-libserialport --without-libftdi --without-libhidapi
--without-libbluez --without-libnettle
--without-librevisa --without-libgpib --without-libieee1284
CFLAGS="-ffunction-sections -fdata-sections -O2 [+ -DGLIB_STATIC_COMPILATION ... on Windows]"
```

`-ffunction-sections -fdata-sections` + cargo release's `--gc-sections` lets
the linker drop unused libsigrok functions at sub-`.o` granularity —
critical because `scpi.c` / `device.c` contain helpers that other .o files
reference, so we can't just `ar d` those whole objects.

Windows additionally needs `-DGLIB_STATIC_COMPILATION` (and siblings) to
prevent GLib headers from marking symbols as `__declspec(dllimport)` —
which would prevent linking against the static `.a`.

## glib meson configure

```
meson setup build --prefix=$OUT_DIR/prefix \
    --buildtype=release \
    --default-library=static \
    -Dtests=false -Dinstalled_tests=false \
    -Dnls=disabled             # no libintl/libcharset dep
    -Dglib_debug=disabled \
    -Dintrospection=disabled \
    -Dman-pages=disabled \
    -Ddocumentation=false \
    -Dsysprof=disabled \
    -Dselinux=disabled \
    -Dlibmount=disabled \
    -Dxattr=false
```

Build output: `libglib-2.0.a`, `libgio-2.0.a`, `libgobject-2.0.a`,
`libgmodule-2.0.a` in `$prefix/lib/`.

## Troubleshooting

### First build fails: "meson: command not found"

Install meson. Linux: `sudo pacman -S meson` / `sudo apt install meson` /
`sudo dnf install meson`. Windows MSYS2: `pacman -S mingw-w64-ucrt-x86_64-meson`.
macOS: `brew install meson`.

### Build says a library is missing after its bootstrap "succeeded"

Inspect `$OUT_DIR/prefix/lib/` to see what actually landed:

```console
$ ls target/release/build/libsigrok-sys-*/out/prefix/lib
libffi.a  libglib-2.0.a  libsigrok.a  ...
```

If a `.a` is missing, re-run the specific library's configure by deleting
its build dir under `$OUT_DIR/src/<lib>-<ver>/` and `cargo build` again.

### GFW / blocked github.com

```bash
LIBSIGROK_SYS_GITHUB_MIRROR=https://ghproxy.com cargo build -p sigrok-bridge
```

The mirror prefix is prepended to any `https://github.com/...` URL the
bootstrap downloads.

### Windows: "non-system DLL dependency" after build

Some static archive was missing and the linker silently fell back to the
import library. Check `objdump -p sigrok-bridge.exe | grep 'DLL Name'`,
identify the offending DLL, trace it back to the missing `.a` in
`$OUT_DIR/prefix/lib/`.

### Cross: "pacman: command not found" / "fakeroot not found"

Cross requires `pacman` + `fakeroot` on the Linux host. Install:

- Arch:   `sudo pacman -S pacman fakeroot`
- Debian: `sudo apt install pacman-package-manager fakeroot`
- Fedora: `sudo dnf install pacman fakeroot`

### Cross: "unknown argument: -r" from lld

llvm-mingw's ld.lld doesn't support `-r` for PE/COFF. `build/windows.rs`
auto-steers libtool's partial-link step to GNU `ld.bfd` via `LD=<path>`.
If that fails, install binutils' mingw-w64 ld:

- Arch:   `sudo pacman -S mingw-w64-binutils`
- Debian: `sudo apt install mingw-w64`

### Cross: `libunwind.dll` appears in final .exe

`build/windows.rs` searches common llvm-mingw install prefixes
(`/opt/llvm-mingw-ucrt/bin`, `/opt/llvm-mingw/bin`, …) for `libunwind.a`
and auto-writes a `static-shim` dir so ld picks the static archive. If
yours lives elsewhere, symlink one of those prefixes or extend
`find_llvm_mingw_libunwind_a`.

### `cargo clean` re-downloaded everything

Tarballs cache is OUTSIDE `$OUT_DIR` (see "Cache layout"), so `cargo clean`
only wipes compiled artifacts. Downloads are reused. To force fully fresh
pull: `rm -rf ~/.cache/libsigrok-sys/`.

### First build takes too long

~5 min on Linux, ~10 min on Windows. Most of the time is spent in
glib's meson compile (~3 min). Subsequent builds are seconds. If you
work on libsigrok itself, edit `$OUT_DIR/src/libsigrok-<commit>/` in
place and `cargo build` — rebuilds just that library.
