//! Raw FFI bindings for libsigrok (and the GLib helpers libsigrok's API
//! exposes), linked at build time via `extern "C"`.
//!
//! ## Linking
//!
//! This is a `-sys` crate (`links = "sigrok"` in Cargo.toml). `build.rs` is
//! the single source of truth for how libsigrok gets linked: it unconditionally
//! bootstraps libsigrok + libffi + zlib + libusb + libzip + pcre2 + glib2
//! (+ libiconv on Windows) from source into `$OUT_DIR/prefix/` and links
//! them statically. Plain `cargo build -p sigrok-bridge` produces a self-
//! contained binary with only the host OS baseline as runtime deps (glibc
//! family on Linux, Win32 + UCRT on Windows, libSystem + frameworks on macOS).
//!
//! Three native platform entries share the same `build/deps.rs` library
//! compilation logic:
//!   * `build/linux.rs`   — Linux host (Arch / Debian / Fedora / …)
//!   * `build/windows.rs` — Windows host running in an MSYS2 UCRT64 shell
//!   * `build/macos.rs`   — macOS host with Xcode + Homebrew
//!
//! No cross-compile is supported — each host compiles natively for itself.
//!
//! ## Knobs
//!
//! * `LIBSIGROK_COMMIT`            — pin libsigrok to a specific git SHA
//!                                   (default: tracked constant in build/common.rs).
//! * `LIBSIGROK_SYS_CACHE_DIR`     — persistent tarball cache root
//!                                   (default: `~/.cache/libsigrok-sys/`).
//! * `LIBSIGROK_SYS_GITHUB_MIRROR` — ghproxy-style prefix for GitHub URLs.
//!
//! ## API surface
//!
//! This crate re-exports only the FFI that `sigrok-bridge` needs. It is
//! *not* a complete binding of libsigrok.

use std::ffi::{c_char, c_int, c_void};

// --- Opaque types ---

#[repr(C)]
pub struct SrContext {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SrSession {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SrDevInst {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SrChannelGroup {
    _private: [u8; 0],
}

#[repr(C)]
pub struct GVariant {
    _private: [u8; 0],
}

// --- GSList (singly-linked list from GLib) ---

#[repr(C)]
pub struct GSList {
    pub data: *mut c_void,
    pub next: *mut GSList,
}

// --- sr_channel (public layout, accessed directly) ---

#[repr(C)]
pub struct SrChannel {
    pub sdi: *mut SrDevInst,
    pub index: c_int,
    pub type_: c_int,
    pub enabled: c_int, // gboolean
    pub name: *mut c_char,
    pub priv_: *mut c_void,
}

// --- sr_dev_driver (we only need it as a pointer) ---

#[repr(C)]
pub struct SrDevDriver {
    pub name: *const c_char,
    pub longname: *const c_char,
    pub api_version: c_int,
    // remaining fields are function pointers we don't call directly
    _opaque: [u8; 0],
}

// --- Datafeed types ---

#[repr(C)]
pub struct SrDatafeedPacket {
    pub type_: u16,
    pub payload: *const c_void,
}

#[repr(C)]
pub struct SrDatafeedLogic {
    pub length: u64,
    pub unitsize: u16,
    pub data: *mut c_void,
}

pub type SrDatafeedCallback = Option<
    unsafe extern "C" fn(
        sdi: *const SrDevInst,
        packet: *const SrDatafeedPacket,
        cb_data: *mut c_void,
    ),
>;

// --- Constants ---

pub const SR_OK: c_int = 0;

// Channel types
pub const SR_CHANNEL_LOGIC: c_int = 10000;
pub const SR_CHANNEL_ANALOG: c_int = 10001;

// Packet types
pub const SR_DF_HEADER: u16 = 10000;
pub const SR_DF_END: u16 = 10001;
pub const SR_DF_TRIGGER: u16 = 10003;
pub const SR_DF_LOGIC: u16 = 10004;

// Config keys
pub const SR_CONF_SAMPLERATE: u32 = 30000;
pub const SR_CONF_PATTERN_MODE: u32 = 30002;
pub const SR_CONF_NUM_LOGIC_CHANNELS: u32 = 30027;
pub const SR_CONF_LIMIT_SAMPLES: u32 = 50001;

// Trigger match types (enum sr_trigger_matches in libsigrok.h)
pub const SR_TRIGGER_ZERO: c_int = 1;
pub const SR_TRIGGER_ONE: c_int = 2;
pub const SR_TRIGGER_RISING: c_int = 3;
pub const SR_TRIGGER_FALLING: c_int = 4;
pub const SR_TRIGGER_EDGE: c_int = 5; // any edge

// --- Opaque trigger types ---

#[repr(C)]
pub struct SrTrigger {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SrTriggerStage {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SrTriggerMatch {
    _private: [u8; 0],
}

// --- Statically-linked C functions ---

extern "C" {
    // Lifecycle
    pub fn sr_init(ctx: *mut *mut SrContext) -> c_int;
    pub fn sr_exit(ctx: *mut SrContext) -> c_int;

    // Driver
    pub fn sr_driver_list(ctx: *const SrContext) -> *mut *mut SrDevDriver;
    pub fn sr_driver_init(ctx: *mut SrContext, driver: *mut SrDevDriver) -> c_int;
    pub fn sr_driver_scan(driver: *mut SrDevDriver, options: *mut GSList) -> *mut GSList;

    // Device
    pub fn sr_dev_open(sdi: *mut SrDevInst) -> c_int;
    pub fn sr_dev_close(sdi: *mut SrDevInst) -> c_int;
    pub fn sr_dev_inst_vendor_get(sdi: *const SrDevInst) -> *const c_char;
    pub fn sr_dev_inst_model_get(sdi: *const SrDevInst) -> *const c_char;
    pub fn sr_dev_inst_version_get(sdi: *const SrDevInst) -> *const c_char;
    pub fn sr_dev_inst_sernum_get(sdi: *const SrDevInst) -> *const c_char;
    pub fn sr_dev_inst_channels_get(sdi: *const SrDevInst) -> *mut GSList;
    pub fn sr_dev_channel_enable(ch: *mut SrChannel, enable: c_int) -> c_int;

    // Session
    pub fn sr_session_new(ctx: *mut SrContext, session: *mut *mut SrSession) -> c_int;
    pub fn sr_session_destroy(session: *mut SrSession) -> c_int;
    pub fn sr_session_dev_add(session: *mut SrSession, sdi: *mut SrDevInst) -> c_int;
    pub fn sr_session_datafeed_callback_add(
        session: *mut SrSession,
        cb: SrDatafeedCallback,
        cb_data: *mut c_void,
    ) -> c_int;
    pub fn sr_session_start(session: *mut SrSession) -> c_int;
    pub fn sr_session_run(session: *mut SrSession) -> c_int;
    pub fn sr_session_stop(session: *mut SrSession) -> c_int;

    // Configuration
    pub fn sr_config_get(
        driver: *const SrDevDriver,
        sdi: *const SrDevInst,
        cg: *const SrChannelGroup,
        key: u32,
        data: *mut *mut GVariant,
    ) -> c_int;
    pub fn sr_config_set(
        sdi: *const SrDevInst,
        cg: *const SrChannelGroup,
        key: u32,
        data: *mut GVariant,
    ) -> c_int;
    pub fn sr_config_list(
        driver: *const SrDevDriver,
        sdi: *const SrDevInst,
        cg: *const SrChannelGroup,
        key: u32,
        data: *mut *mut GVariant,
    ) -> c_int;

    // Trigger
    pub fn sr_trigger_new(name: *const c_char) -> *mut SrTrigger;
    pub fn sr_trigger_free(trig: *mut SrTrigger);
    pub fn sr_trigger_stage_add(trig: *mut SrTrigger) -> *mut SrTriggerStage;
    pub fn sr_trigger_match_add(
        stage: *mut SrTriggerStage,
        ch: *mut SrChannel,
        trigger_match: c_int,
        value: f32,
    ) -> c_int;
    pub fn sr_session_trigger_set(
        session: *mut SrSession,
        trig: *mut SrTrigger,
    ) -> c_int;

    // GLib helpers
    pub fn g_variant_new_string(s: *const c_char) -> *mut GVariant;
    pub fn g_variant_new_uint64(v: u64) -> *mut GVariant;
    pub fn g_variant_new_int32(v: i32) -> *mut GVariant;
    pub fn g_variant_get_uint64(v: *mut GVariant) -> u64;
    pub fn g_variant_unref(v: *mut GVariant);
    pub fn g_variant_get_type_string(v: *mut GVariant) -> *const c_char;
    pub fn g_variant_n_children(v: *mut GVariant) -> usize;
    pub fn g_variant_get_child_value(v: *mut GVariant, index: usize) -> *mut GVariant;
    pub fn g_variant_get_variant(v: *mut GVariant) -> *mut GVariant;
    pub fn g_slist_free(list: *mut GSList);
}

/// Iterate a GSList, yielding data pointers.
///
/// The returned pointers are borrowed from the list nodes and must NOT be
/// freed by the caller (e.g. channel lists are owned by the device instance).
pub unsafe fn gslist_iter(mut list: *mut GSList) -> Vec<*mut c_void> {
    let mut result = Vec::new();
    while !list.is_null() {
        result.push((*list).data);
        list = (*list).next;
    }
    result
}
