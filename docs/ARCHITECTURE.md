# Architecture

## System Overview

```
+------------------+     twinlan TCP      +-------------------+     C FFI      +-----------+
|  ngscopeclient   | ===================> | sigrok-bridge     | =============> | libsigrok |
|  (C++)           |  cmd: SCPI text      | (Rust)            |  sr_* API      | (C)       |
|                  |  data: binary wfm    |                   |                |           |
| SigrokOscilloscope                      | libsigrok-sys      (FFI + linkage)  | Hardware  |
| RemoteBridgeOscil|                      | sigrok-bridge-common                | driver    |
+------------------+                      | - SCPI parser                       +-----------+
                                          | - twinlan server
                                          | - protocol defs
                                          +-------------------+
```

## Components

### SigrokOscilloscope (C++ Driver)

- Inherits `RemoteBridgeOscilloscope` in scopehal
- Registered as driver name `"sigrok"` with transport `"twinlan"`
- Sends SCPI commands on the command socket
- Receives binary waveform data on the data socket
- Supports two ADC modes: Digital (LA) and 8-bit Analog (OSC)

### libsigrok-sys (Rust `-sys` crate)

Single source of truth for libsigrok linkage in the workspace (`links =
"sigrok"` in Cargo.toml).

- `src/lib.rs`: raw `extern "C"` FFI — opaque types, constants, `sr_*` fn
  declarations, and the GLib helpers libsigrok's API exposes
- `build.rs`: target dispatcher — picks Linux / Windows / macOS module
- `build/common.rs`: shared helpers (cache dir, fetch, extract, tool checks)
- `build/deps.rs`: per-library build functions, parameterized by `Platform`.
  Compiles libffi, zlib, libusb, libzip, pcre2, libiconv (Windows only),
  glib2, libsigrok from source.
- `build/linux.rs`, `build/windows.rs`, `build/macos.rs`: per-platform
  entry points — tool-chain check, invoke `deps::build_all`, emit the
  target's link directives (pkg-config-driven on Linux/macOS, hardcoded
  list on Windows).

All three platforms patch out libsigrok's VXI-11 SCPI backend to avoid
pulling libtirpc / krb5 onto the link line. See `BUILD.md` for the full
walk-through.

### sigrok-bridge-common (Rust Library)

Shared code used by both bridge servers:

- **`scpi.rs`**: `ScpiCommand` enum and `parse_scpi_line()` parser
- **`twinlan.rs`**: `TwinLanServer` (dual TCP socket listener) and `TwinLanConnection`
- **`protocol.rs`**: `WaveformHeader` struct with 38-byte LE serialization
- **`types.rs`**: `DeviceInfo`, `ChannelConfig`, coupling/trigger types

### fake-bridge (Rust Binary)

Test server that emulates a 16-channel logic analyzer without real hardware:

- Responds to all SCPI queries with fake device info
- Generates deterministic square wave patterns (channel N toggles at 2^(N+1) period)
- Implements the full acquisition state machine (start/stop/single/force/armed)
- Useful for developing and testing the C++ driver without hardware

### sigrok-bridge (Rust Binary)

Production bridge server that wraps a real sigrok-compatible device:

- Depends on `libsigrok-sys` for the FFI; no `extern "C"` in this crate
- Discovers and opens a device by driver name. Only `sipeed-slogic-analyzer`
  is available (libsigrok is built with `--disable-all-drivers --enable-sipeed-slogic-analyzer`).
  To enable other drivers, edit the configure flags in `libsigrok-sys/build/linux.rs`
  / `build/windows.rs`.
- Maps SCPI commands to libsigrok `sr_config_set`/`sr_config_get` calls
- Runs `sr_session_run()` in a thread; datafeed callback collects `SR_DF_LOGIC` packets
- Converts sigrok's bit-packed logic data directly to the binary protocol format

## Data Flow

### Acquisition Sequence

```
ngscopeclient                    bridge server                hardware
     |                               |                          |
     |-- START ---------------------->|                          |
     |                               |-- sr_config_set(rate) -->|
     |                               |-- sr_config_set(depth)->|
     |                               |                          |
     |-- 'K' (data socket) --------->|                          |
     |                               |-- sr_session_start() --->|
     |                               |<--- SR_DF_LOGIC ---------|
     |                               |<--- SR_DF_TRIGGER -------|
     |                               |<--- SR_DF_LOGIC ---------|
     |                               |<--- SR_DF_END -----------|
     |<-- WaveformHeader (38 bytes) --|                          |
     |<-- data chunks (256K samples) -|                          |
     |                               |                          |
     |-- STOP ----------------------->|                          |
     |                               |-- sr_session_stop() ---->|
```

### Channel Mapping

- **Digital mode**: sigrok logic channels D0..Dn map 1:1 to digital channels
- **Analog mode**: every 8 digital channels are grouped into one analog channel
  - D0-D7 -> A0 (8-bit ADC)
  - D8-D15 -> A1 (8-bit ADC)
  - Voltage: `(raw_byte / 255.0) * range + offset`
