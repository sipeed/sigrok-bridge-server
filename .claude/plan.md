# Plan: Static-link libsigrok + glib — kill all dynamic loading

## Goal
Replace all `libloading` runtime loading with direct `extern "C"` declarations.
Both libsigrok and glib symbols resolved by the linker at build time (static `.a`
archives). User supplies `include/` and `lib/` directories; build.rs finds them
via env vars or pkg-config fallback. Same approach for Linux and Windows.

## Files to change

### 1. `sigrok-bridge/Cargo.toml`
- **Remove** `libloading = "0.8"` dependency
- **Add** `pkg-config = "0.3"` as a **build** dependency

### 2. `sigrok-bridge/build.rs` — complete rewrite
- **Priority 1**: `SIGROK_LIB_DIR` / `SIGROK_INCLUDE_DIR` env vars
- **Fallback**: `pkg-config --static --libs libsigrok glib-2.0`
- Emit: `cargo:rustc-link-search=native=...`, `cargo:rustc-link-lib=static=sigrok`,
  `cargo:rustc-link-lib=static=glib-2.0`, plus transitive deps (intl, iconv,
  pcre2-8, usb-1.0, zip, z, ffi, etc.)
- On Windows: also emit system import libs (ws2_32, ole32, winmm, shlwapi, uuid,
  setupapi, iphlpapi)
- `cargo:rerun-if-env-changed=SIGROK_LIB_DIR`

### 3. `sigrok-bridge/src/ffi.rs` — complete rewrite
- **Delete** everything related to `libloading`, `Library`, `Symbol`, `load_sym!`,
  `Sigrok::load()`, `load_sigrok_library()`, `load_glib_library()`,
  `mod static_glib`, `_lib` / `_glib_lib` fields
- **Keep** all type definitions (opaque types, GSList, SrChannel, SrDevDriver,
  SrDatafeedPacket, SrDatafeedLogic, SrDatafeedCallback, constants)
- **Add** a single `extern "C"` block with all 30 functions:
  ```rust
  extern "C" {
      pub fn sr_init(...) -> c_int;
      pub fn sr_exit(...) -> c_int;
      // ... all 21 sr_* functions ...
      pub fn g_variant_new_string(...) -> *mut GVariant;
      // ... all 9 g_* functions ...
  }
  ```
- **Keep** `gslist_iter()` utility unchanged
- **Delete** `Sigrok` struct entirely — it was just a bag of function pointers +
  library handles, now unnecessary

### 4. `sigrok-bridge/src/sigrok_device.rs` — mechanical replacement
- Remove `sigrok: &'static Sigrok` field from `SigrokDevice`
- Remove `sigrok` parameter from `SigrokDevice::open()`
- Replace every `(self.sigrok.sr_foo)(args)` → `ffi::sr_foo(args)`
- Replace every `(self.sigrok.g_variant_foo)(args)` → `ffi::g_variant_foo(args)`
- Replace every `(self.sigrok.g_slist_free)(args)` → `ffi::g_slist_free(args)`
- Drop impl: `(self.sigrok.sr_dev_close)(...)` → `ffi::sr_dev_close(...)`

### 5. `sigrok-bridge/src/main.rs`
- Remove `use ffi::Sigrok;`
- Remove `--lib-path` CLI argument
- Remove `Sigrok::load(...)` call and `Box::leak(...)` line
- `SigrokDevice::open(sigrok, ...)` → `SigrokDevice::open(...)` (no sigrok param)

### 6. `.cargo/config.toml`
- Keep gnullvm target config (still needed for Windows cross-compile)
- Remove windows-gnu target section (no longer a useful path without libloading)

## Build invocations

**Linux (native)**:
```bash
cargo build --release -p sigrok-bridge
# pkg-config finds libsigrok + glib2 automatically
```

**Linux (explicit paths)**:
```bash
SIGROK_LIB_DIR=/path/to/lib SIGROK_INCLUDE_DIR=/path/to/include \
  cargo build --release -p sigrok-bridge
```

**Windows (cross)**:
```bash
SIGROK_LIB_DIR=/path/to/win-lib \
  cargo build --release --target x86_64-pc-windows-gnullvm -p sigrok-bridge
```
