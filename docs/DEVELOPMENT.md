# Development Guide

## Prerequisites

- Rust toolchain (rustup, cargo)
- CMake + Ninja (for scopehal-apps)
- Build toolchain for `libsigrok-sys` bootstrap (autoconf / automake / libtool /
  pkg-config / curl / tar / gcc). `build.rs` checks these up-front with
  actionable error messages.
- libsigrok: no separate install needed — `libsigrok-sys/build.rs` downloads
  the Sipeed fork (`LIBSIGROK_COMMIT` pin) and statically links it into the
  final binary. See `BUILD.md` for advanced knobs.

## Project Structure

```
ngscope-ws-ng/
  scopehal-apps/              # ngscopeclient + scopehal library (C++)
    lib/scopehal/
      SigrokOscilloscope.h    # Driver header
      SigrokOscilloscope.cpp  # Driver implementation
  sigrok-bridge-server/       # Rust workspace
    libsigrok-sys/            # -sys crate: FFI + bootstrap libsigrok → static link
    sigrok-bridge-common/     # Shared library (SCPI parser, twinlan, protocol)
    fake/                     # Fake bridge server for testing
    sigrok-bridge/            # Real bridge server (depends on libsigrok-sys)
    docs/                     # Documentation
```

The `libsigrok-sys` sub-crate is the single source of truth for libsigrok
linkage (`links = "sigrok"` in its Cargo.toml). Downstream crates that want
to call libsigrok symbols depend on it — they cannot emit their own
`cargo:rustc-link-lib=sigrok`.

## Building

### scopehal-apps (C++ driver)

```bash
cd scopehal-apps
cmake -GNinja -DCMAKE_EXPORT_COMPILE_COMMANDS=ON -DBUILD_DOCS=OFF \
  -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=OFF -Bbuild
cmake --build build
```

### Rust bridge servers

```bash
cd sigrok-bridge-server

# Build all
cargo build

# Build only fake-bridge
cargo build -p fake-bridge

# Build only sigrok-bridge
cargo build -p sigrok-bridge

# Run tests
cargo test
```

## Testing with fake-bridge

1. Start the fake bridge server:
   ```bash
   cargo run -p fake-bridge -- --port 10101
   ```
   This starts a server on port 10101 (command) and 10102 (data), emulating a 16-channel logic analyzer.

2. Launch ngscopeclient and connect:
   ```
   Driver: sigrok
   Transport: twinlan
   Arguments: localhost:10101
   ```

3. The fake bridge generates deterministic square wave patterns:
   - Channel D0: toggles every 2 samples
   - Channel D1: toggles every 4 samples
   - Channel DN: toggles every 2^(N+1) samples

## Running the real bridge

`libsigrok-sys` self-bootstraps libsigrok, so no system install is required:

```bash
cargo run --release -p sigrok-bridge
# → listens on 10101/10102, uses the sipeed-slogic-analyzer driver
```

First run spends ~5 min compiling libsigrok + deps. Output is a fully static
binary (`ldd` shows only glibc family + linux-vdso).

CLI options:

| Flag                    | Default                  | Description                          |
|-------------------------|--------------------------|--------------------------------------|
| `-d`, `--driver <NAME>` | `sipeed-slogic-analyzer` | libsigrok driver name                |
| `-p`, `--port <PORT>`   | `10101`                  | TCP command port (data = port + 1)   |
| `--pattern-mode <MODE>` | (unset)                  | `Normal` / `"USB connection test"` / `Emulation` |
| `--adc-mode <MODE>`     | `digital`                | `digital` (LA) or `analog` (group 8 lines as 8-bit ADC) |

Connect from ngscopeclient as described above.

## ADC Mode

The SigrokOscilloscope driver supports two ADC modes:

- **Digital (mode 0)**: Standard logic analyzer mode. Each channel is a digital signal displayed as a sparse digital waveform.
- **8-bit Analog (mode 1)**: Reinterprets every 8 digital channels as one 8-bit ADC value. Useful for devices like SLogic16U3 with external ADC modules.

Switch modes in ngscopeclient via the channel configuration panel.

## Debugging

Enable verbose logging:
```bash
RUST_LOG=debug cargo run -p fake-bridge -- --port 10101
RUST_LOG=trace cargo run -p sigrok-bridge
```
