//! Wrapper around libsigrok device instance, providing a safe-ish API
//! for the bridge server to configure and run acquisitions.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sigrok_bridge_common::device::{AcquisitionData, BridgeDevice};
use sigrok_bridge_common::trigger::{find_trigger_analog, find_trigger_digital};
use sigrok_bridge_common::types::{
    parse_channel_hwname, ChannelGroup, DeviceInfo, TriggerConfig, TriggerEdgeDir,
};

/// Snapshot of the trigger-config-dependent identity used to decide whether
/// the persisted cross-buffer `last_trigger_state` is still valid. If any of
/// these change, the previous state is meaningless and must be reset.
#[derive(Debug, Clone, PartialEq)]
struct TriggerStateKey {
    source: Option<String>,
    edge_dir: TriggerEdgeDir,
    level: f32,
}

use libsigrok_sys as ffi;
use libsigrok_sys::*;

/// Data collected from the datafeed callback during one acquisition.
/// Private to this module — converted to the public `AcquisitionData` in `acquire()`.
struct CallbackData {
    logic_data: Vec<u8>,
    unitsize: u16,
    triggered: bool,
    trigger_sample: u64,
}

impl CallbackData {
    fn new() -> Self {
        Self {
            logic_data: Vec::new(),
            unitsize: 0,
            triggered: false,
            trigger_sample: 0,
        }
    }
}

/// Shared state passed to the datafeed callback.
struct CallbackState {
    acquisition: Mutex<CallbackData>,
}

/// A sigrok device wrapper that manages the lifecycle of a device instance.
pub struct SigrokDevice {
    ctx: *mut SrContext,
    session: *mut SrSession,
    sdi: *mut SrDevInst,
    driver: *mut SrDevDriver,

    info: DeviceInfo,
    /// Ordered per-byte channel layout (one entry per byte of the capture
    /// width, ascending byte_offset). Source of truth for CHANS?/LAYOUT? and
    /// the derived analog/digital channel counts.
    channel_layout: Vec<ChannelGroup>,
    /// Physical capture width in channels (= max_channels). Waveform header
    /// num_channels and unitsize (= max_channels/8) derive from this; the raw
    /// data is always bit-packed and routed to channels client-side.
    hw_digital_channel_count: u16,
    /// Samples kept before the trigger point (0 = trigger at the left edge,
    /// post-trigger only). Applied by cropping the buffer in acquire().
    pretrigger: u64,
    pub sample_rates: Vec<u64>,
    pub sample_depths: Vec<u64>,

    sample_rate: AtomicU64,
    sample_depth: AtomicU64,

    // Acquisition state machine
    running: AtomicBool,
    one_shot: AtomicBool,
    force_trigger: AtomicBool,
    trigger_armed: AtomicBool,
    shutdown: AtomicBool,
    seqnum: AtomicU32,

    trigger_config: Mutex<TriggerConfig>,

    /// Persistent trigger state across acquisitions — lets us detect rising/falling
    /// edges that straddle the gap between two sr_session_run() calls. Tuple is
    /// (identity_of_trigger_config_when_captured, last_level_above_threshold_or_bit_high).
    /// Reset on Start/Single/Force (new session) or when trigger config changes.
    last_trigger_state: Mutex<Option<(TriggerStateKey, bool)>>,

    cb_state: Arc<CallbackState>,
    /// Raw pointer from Arc::into_raw, recovered in Drop
    cb_raw_ptr: *mut std::ffi::c_void,
}

// Safety: The sigrok C API is not thread-safe in general, but we carefully
// synchronize access. The raw pointers are used from controlled contexts only.
unsafe impl Send for SigrokDevice {}
unsafe impl Sync for SigrokDevice {}

impl SigrokDevice {
    /// Open a sigrok device by driver name.
    ///
    /// `driver_name` is e.g. "sipeed-slogic-analyzer" or "fx2lafw".
    /// `device_index` selects which device if multiple are found (0 = first).
    /// `max_channels`: physical capture width (8/16/32); `None` uses the
    /// device's hardware logic count. Sets SR_CONF_NUM_LOGIC_CHANNELS so the
    /// driver's rate ceiling follows.
    /// `analog_groups`: byte-group indices to expose as 8-bit analog channels;
    /// all other groups in 0..max_channels/8 are digital. Indices outside that
    /// range are logged and ignored.
    pub fn open(
        driver_name: &str,
        device_index: usize,
        max_channels: Option<u32>,
        analog_groups: &[usize],
        pretrigger: u64,
    ) -> Result<Self, String> {
        unsafe {
            // Initialize context
            let mut ctx: *mut SrContext = ptr::null_mut();
            let ret = ffi::sr_init(&mut ctx);
            if ret != SR_OK {
                return Err(format!("sr_init failed: {}", ret));
            }

            // Find driver
            let drivers = ffi::sr_driver_list(ctx);
            if drivers.is_null() {
                return Err("No drivers available".into());
            }

            let mut driver: *mut SrDevDriver = ptr::null_mut();
            let mut i = 0;
            loop {
                let drv = *drivers.add(i);
                if drv.is_null() {
                    break;
                }
                let name = CStr::from_ptr((*drv).name);
                if name.to_str().unwrap_or("") == driver_name {
                    driver = drv;
                    break;
                }
                i += 1;
            }

            if driver.is_null() {
                // List available drivers for error message
                let mut available = Vec::new();
                let mut j = 0;
                loop {
                    let drv = *drivers.add(j);
                    if drv.is_null() {
                        break;
                    }
                    if let Ok(n) = CStr::from_ptr((*drv).name).to_str() {
                        available.push(n.to_string());
                    }
                    j += 1;
                }
                return Err(format!(
                    "Driver '{}' not found. Available: {}",
                    driver_name,
                    available.join(", ")
                ));
            }

            // Initialize driver
            let ret = ffi::sr_driver_init(ctx, driver);
            if ret != SR_OK {
                return Err(format!("sr_driver_init failed: {}", ret));
            }

            // Scan for devices
            let dev_list = ffi::sr_driver_scan(driver, ptr::null_mut());
            let devices = gslist_iter(dev_list);
            if devices.is_empty() {
                return Err(format!("No devices found for driver '{}'", driver_name));
            }

            if device_index >= devices.len() {
                return Err(format!(
                    "Device index {} out of range (found {} devices)",
                    device_index,
                    devices.len()
                ));
            }

            let sdi = devices[device_index] as *mut SrDevInst;

            // Open device
            let ret = ffi::sr_dev_open(sdi);
            if ret != SR_OK {
                return Err(format!("sr_dev_open failed: {}", ret));
            }

            // Get device info
            let vendor = cstr_or_unknown(ffi::sr_dev_inst_vendor_get(sdi));
            let model = cstr_or_unknown(ffi::sr_dev_inst_model_get(sdi));
            let serial = cstr_or_unknown(ffi::sr_dev_inst_sernum_get(sdi));
            let firmware = cstr_or_unknown(ffi::sr_dev_inst_version_get(sdi));

            // Count physical channels from libsigrok
            let ch_list = ffi::sr_dev_inst_channels_get(sdi);
            let channels = gslist_iter(ch_list);
            let mut hw_logic_count = 0u16;
            let mut hw_analog_count = 0u16;
            for ch_ptr in &channels {
                let ch = *ch_ptr as *const SrChannel;
                match (*ch).type_ {
                    SR_CHANNEL_LOGIC => hw_logic_count += 1,
                    SR_CHANNEL_ANALOG => hw_analog_count += 1,
                    _ => {}
                }
            }

            let _ = hw_analog_count;

            // Resolve the physical capture width. Default to the hardware logic
            // count; otherwise honor the caller's request.
            let max_channels = max_channels.unwrap_or(hw_logic_count as u32);
            if max_channels == 0 || max_channels % 8 != 0 {
                return Err(format!(
                    "Invalid max_channels {} (must be a positive multiple of 8)",
                    max_channels
                ));
            }

            // Tell the driver how many logic channels to capture BEFORE rates
            // and depths are queried, so its rate ceiling follows the width.
            let variant = ffi::g_variant_new_int32(max_channels as i32);
            let ret = ffi::sr_config_set(
                sdi,
                ptr::null(),
                SR_CONF_NUM_LOGIC_CHANNELS,
                variant,
            );
            if ret != SR_OK {
                log::warn!(
                    "Failed to set NUM_LOGIC_CHANNELS={} (ret={}); driver may clamp",
                    max_channels, ret
                );
            }

            // Build the ordered per-byte channel layout: one ChannelGroup per
            // byte of the capture width, analog where requested, else digital.
            let num_groups = (max_channels / 8) as usize;
            let mut channel_layout: Vec<ChannelGroup> = Vec::with_capacity(num_groups);
            for g in 0..num_groups {
                let analog = analog_groups.contains(&g);
                channel_layout.push(ChannelGroup {
                    analog,
                    byte_offset: g,
                    name: if analog {
                        format!("A{}", g)
                    } else {
                        format!("D{}", 8 * g)
                    },
                });
            }
            for &g in analog_groups {
                if g >= num_groups {
                    log::warn!(
                        "Ignoring analog group A{} (out of range for {} byte-groups)",
                        g, num_groups
                    );
                }
            }

            let analog_count = channel_layout.iter().filter(|g| g.analog).count();
            let digital_group_count = num_groups - analog_count;
            log::info!(
                "Channel layout: {} bytes ({} analog groups, {} digital groups = {} logic lines)",
                num_groups, analog_count, digital_group_count, 8 * digital_group_count
            );

            // Create session
            let mut session: *mut SrSession = ptr::null_mut();
            let ret = ffi::sr_session_new(ctx, &mut session);
            if ret != SR_OK {
                return Err(format!("sr_session_new failed: {}", ret));
            }

            let ret = ffi::sr_session_dev_add(session, sdi);
            if ret != SR_OK {
                return Err(format!("sr_session_dev_add failed: {}", ret));
            }

            let cb_state = Arc::new(CallbackState {
                acquisition: Mutex::new(CallbackData::new()),
            });

            // Register datafeed callback once (not per-acquire)
            let cb_ptr = Arc::into_raw(Arc::clone(&cb_state)) as *mut std::ffi::c_void;
            let ret = ffi::sr_session_datafeed_callback_add(
                session,
                Some(datafeed_callback),
                cb_ptr,
            );
            if ret != SR_OK {
                let _ = Arc::from_raw(cb_ptr as *const CallbackState);
                return Err(format!("sr_session_datafeed_callback_add failed: {}", ret));
            }

            // Free scan results list (but not the devices themselves)
            if !dev_list.is_null() {
                ffi::g_slist_free(dev_list);
            }

            log::info!(
                "Opened device: {} {} (serial: {}, fw: {}), capture width {} channels",
                vendor, model, serial, firmware, max_channels
            );

            Ok(Self {
                ctx,
                session,
                sdi,
                driver,
                info: DeviceInfo {
                    vendor,
                    model,
                    serial,
                    firmware,
                },
                channel_layout,
                hw_digital_channel_count: max_channels as u16,
                pretrigger,
                sample_rates: Vec::new(),
                sample_depths: Vec::new(),
                sample_rate: AtomicU64::new(0),
                sample_depth: AtomicU64::new(0),
                running: AtomicBool::new(false),
                one_shot: AtomicBool::new(false),
                force_trigger: AtomicBool::new(false),
                trigger_armed: AtomicBool::new(false),
                shutdown: AtomicBool::new(false),
                seqnum: AtomicU32::new(0),
                trigger_config: Mutex::new(TriggerConfig::default()),
                last_trigger_state: Mutex::new(None),
                cb_state,
                cb_raw_ptr: cb_ptr,
            })
        }
    }

    /// Set the pattern mode (e.g. "Normal", "USB connection test", "Emulation").
    pub fn set_pattern_mode(&self, mode: &str) -> Result<(), String> {
        let cmode = CString::new(mode).map_err(|e| format!("Invalid mode string: {}", e))?;
        unsafe {
            let variant = ffi::g_variant_new_string(cmode.as_ptr());
            let ret = ffi::sr_config_set(
                self.sdi,
                ptr::null(),
                SR_CONF_PATTERN_MODE,
                variant,
            );
            if ret != SR_OK {
                return Err(format!("Failed to set pattern mode '{}': {}", mode, ret));
            }
            log::info!("Pattern mode set to '{}'", mode);
            Ok(())
        }
    }

    /// Stop the current session (can be called from another thread).
    pub fn stop_session(&self) {
        unsafe {
            let _ = ffi::sr_session_stop(self.session);
        }
    }

    /// Query available sample rates from the hardware.
    pub fn get_available_rates(&self) -> Result<Vec<u64>, String> {
        self.get_config_list_uint64(SR_CONF_SAMPLERATE)
    }

    /// Query available sample depths from the hardware.
    pub fn get_available_depths(&self) -> Result<Vec<u64>, String> {
        self.get_config_list_uint64(SR_CONF_LIMIT_SAMPLES)
    }

    // --- Private helpers ---

    fn get_config_list_uint64(&self, key: u32) -> Result<Vec<u64>, String> {
        unsafe {
            let mut variant: *mut GVariant = ptr::null_mut();
            let ret = ffi::sr_config_list(
                self.driver as *const _,
                self.sdi as *const _,
                ptr::null(),
                key,
                &mut variant,
            );
            if ret != SR_OK {
                return Err(format!("sr_config_list failed for key {}: {}", key, ret));
            }

            let result = parse_gvariant_uint64_list(variant);
            ffi::g_variant_unref(variant);
            if result.is_empty() {
                Err(format!("sr_config_list returned no values for key {}", key))
            } else {
                Ok(result)
            }
        }
    }
}

impl BridgeDevice for SigrokDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn channel_layout(&self) -> &[ChannelGroup] {
        &self.channel_layout
    }

    fn analog_channel_count(&self) -> u16 {
        self.channel_layout.iter().filter(|g| g.analog).count() as u16
    }

    fn digital_channel_count(&self) -> u16 {
        // 8 logic lines per digital byte-group.
        (self.channel_layout.iter().filter(|g| !g.analog).count() * 8) as u16
    }

    fn hw_digital_channel_count(&self) -> u16 {
        self.hw_digital_channel_count
    }

    fn sample_rates(&self) -> &[u64] {
        &self.sample_rates
    }

    fn sample_depths(&self) -> &[u64] {
        &self.sample_depths
    }

    fn start(&self) {
        self.running.store(true, Ordering::SeqCst);
        self.one_shot.store(false, Ordering::SeqCst);
        self.force_trigger.store(false, Ordering::SeqCst);
        self.trigger_armed.store(true, Ordering::SeqCst);
        *self.last_trigger_state.lock().unwrap() = None;
        log::info!("Acquisition started (continuous)");
    }

    fn single(&self) {
        self.running.store(true, Ordering::SeqCst);
        self.one_shot.store(true, Ordering::SeqCst);
        self.force_trigger.store(false, Ordering::SeqCst);
        self.trigger_armed.store(true, Ordering::SeqCst);
        *self.last_trigger_state.lock().unwrap() = None;
        log::info!("Acquisition started (single shot)");
    }

    fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.trigger_armed.store(false, Ordering::SeqCst);
        self.stop_session();
        log::info!("Acquisition stopped");
    }

    fn force(&self) {
        self.force_trigger.store(true, Ordering::SeqCst);
        self.trigger_armed.store(true, Ordering::SeqCst);
        if !self.running.load(Ordering::SeqCst) {
            self.running.store(true, Ordering::SeqCst);
            self.one_shot.store(true, Ordering::SeqCst);
        }
        *self.last_trigger_state.lock().unwrap() = None;
        log::info!("Trigger forced");
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn is_one_shot(&self) -> bool {
        self.one_shot.load(Ordering::SeqCst)
    }

    fn is_force_triggered(&self) -> bool {
        self.force_trigger.load(Ordering::SeqCst)
    }

    fn clear_force_trigger(&self) {
        self.force_trigger.store(false, Ordering::SeqCst);
    }

    fn is_armed(&self) -> bool {
        self.trigger_armed.load(Ordering::SeqCst)
    }

    fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.stop_session();
    }

    fn reset_for_new_connection(&self) {
        self.shutdown.store(false, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        self.one_shot.store(false, Ordering::SeqCst);
        self.force_trigger.store(false, Ordering::SeqCst);
        self.trigger_armed.store(false, Ordering::SeqCst);
        self.seqnum.store(0, Ordering::SeqCst);
        *self.last_trigger_state.lock().unwrap() = None;
    }

    fn next_seqnum(&self) -> u32 {
        self.seqnum.fetch_add(1, Ordering::SeqCst)
    }

    fn complete_one_shot(&self) {
        if self.one_shot.load(Ordering::SeqCst) {
            self.running.store(false, Ordering::SeqCst);
            self.trigger_armed.store(false, Ordering::SeqCst);
        }
    }

    fn set_rate(&self, rate: u64) -> Result<(), String> {
        if !self.sample_rates.contains(&rate) {
            return Err(format!(
                "Unsupported sample rate {} Hz (advertised rates: {})",
                rate,
                self.sample_rates
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }

        unsafe {
            let variant = ffi::g_variant_new_uint64(rate);
            let ret = ffi::sr_config_set(
                self.sdi,
                ptr::null(),
                SR_CONF_SAMPLERATE,
                variant,
            );
            if ret != SR_OK {
                return Err(format!("Failed to set sample rate: {}", ret));
            }

            // Read back the value selected by the driver. Some drivers clamp
            // unsupported values while still returning SR_OK; never report the
            // requested value as active unless it actually took effect.
            let mut actual_variant: *mut GVariant = ptr::null_mut();
            let ret = ffi::sr_config_get(
                self.driver,
                self.sdi,
                ptr::null(),
                SR_CONF_SAMPLERATE,
                &mut actual_variant,
            );
            if ret != SR_OK || actual_variant.is_null() {
                return Err(format!("Failed to read back sample rate: {}", ret));
            }
            let actual = ffi::g_variant_get_uint64(actual_variant);
            ffi::g_variant_unref(actual_variant);
            self.sample_rate.store(actual, Ordering::SeqCst);
            log::info!("Sample rate set to {} Hz", actual);
            Ok(())
        }
    }

    fn set_depth(&self, depth: u64) -> Result<(), String> {
        unsafe {
            let variant = ffi::g_variant_new_uint64(depth);
            let ret = ffi::sr_config_set(
                self.sdi,
                ptr::null(),
                SR_CONF_LIMIT_SAMPLES,
                variant,
            );
            if ret != SR_OK {
                return Err(format!("Failed to set sample depth: {}", ret));
            }
            self.sample_depth.store(depth, Ordering::SeqCst);
            log::info!("Sample depth set to {} samples", depth);
            Ok(())
        }
    }

    fn get_rate(&self) -> u64 {
        self.sample_rate.load(Ordering::SeqCst)
    }

    fn get_depth(&self) -> u64 {
        self.sample_depth.load(Ordering::SeqCst)
    }

    fn set_trigger_source(&self, source: String) {
        log::info!("Trigger source set to {}", source);
        self.trigger_config.lock().unwrap().source_channel = Some(source);
    }

    fn set_trigger_edge_dir(&self, dir: TriggerEdgeDir) {
        log::info!("Trigger edge direction set to {:?}", dir);
        self.trigger_config.lock().unwrap().edge_dir = dir;
    }

    fn set_trigger_level(&self, level: f32) {
        log::info!("Trigger level set to {}", level);
        self.trigger_config.lock().unwrap().level = level;
    }

    fn set_trigger_delay(&self, delay_fs: i64) {
        log::info!("Trigger delay set to {} fs", delay_fs);
        self.trigger_config.lock().unwrap().delay_fs = delay_fs;
    }

    fn get_trigger_delay(&self) -> i64 {
        self.trigger_config.lock().unwrap().delay_fs
    }

    fn get_trigger_config(&self) -> TriggerConfig {
        self.trigger_config.lock().unwrap().clone()
    }

    /// Run a single acquisition: apply trigger, start session, collect data,
    /// and run software trigger fallback if hardware didn't fire.
    fn acquire(&self) -> Result<AcquisitionData, String> {
        // Trigger detection is done entirely in software (find_trigger_* below) on
        // the fully-captured buffer — uniformly for digital, analog, and the mixed
        // per-group layout. We deliberately do NOT install a libsigrok
        // soft_trigger_logic trigger: it emits only post-trigger data and blocks
        // sr_session_run() indefinitely when the matching edge never arrives, and
        // it cannot see the analog channels at all (they are a host-side
        // reinterpretation of the raw logic bytes). Capturing a bounded
        // limit_samples buffer and scanning it is robust and treats every channel
        // type identically.

        unsafe {
            // Reset callback state for this acquisition
            {
                let mut acq = self.cb_state.acquisition.lock().unwrap();
                *acq = CallbackData::new();
            }

            log::info!("acquire: starting session...");
            let ret = ffi::sr_session_start(self.session);
            if ret != SR_OK {
                return Err(format!("sr_session_start failed: {}", ret));
            }

            log::info!("acquire: session started, running (blocking)...");
            let ret = ffi::sr_session_run(self.session);
            if ret != SR_OK {
                return Err(format!("sr_session_run failed: {}", ret));
            }

            let mut result = {
                let acq = self.cb_state.acquisition.lock().unwrap();
                AcquisitionData {
                    logic_data: acq.logic_data.clone(),
                    unitsize: acq.unitsize,
                    triggered: acq.triggered,
                    trigger_sample: acq.trigger_sample,
                }
            };

            // FORCE is a bridge-side trigger operation, so libsigrok does not
            // emit SR_DF_TRIGGER for it. Mark the acquisition here so every
            // layer observes the same effective trigger state.
            if self.force_trigger.load(Ordering::SeqCst) && !result.logic_data.is_empty() {
                result.triggered = true;
                result.trigger_sample = 0;
            }

            // Software trigger fallback: if the hardware didn't report a
            // trigger point, scan the data for the first matching edge. Uses
            // cross-buffer prev_state so edges that fell in the gap between
            // acquisitions still fire at sample 0 of the new buffer.
            if !result.triggered && !result.logic_data.is_empty() {
                let config = self.trigger_config.lock().unwrap().clone();
                if let Some(ref source) = config.source_channel {
                    if let Some((ch_type, ch_index)) = parse_channel_hwname(source) {
                        let us = result.unitsize as usize;
                        let key = TriggerStateKey {
                            source: Some(source.clone()),
                            edge_dir: config.edge_dir,
                            level: config.level,
                        };
                        // Pull the persisted prev_state only if the key still
                        // matches — otherwise reset (treat as first buffer).
                        let prev_state: Option<bool> = {
                            let guard = self.last_trigger_state.lock().unwrap();
                            match &*guard {
                                Some((k, s)) if *k == key => Some(*s),
                                _ => None,
                            }
                        };

                        let scan = match ch_type {
                            'D' | 'd' => Some(find_trigger_digital(
                                &result.logic_data,
                                us,
                                ch_index,
                                config.edge_dir,
                                prev_state,
                            )),
                            'A' | 'a' => {
                                // In 8-bit analog mode, raw SR_DF_LOGIC data is
                                // reinterpreted: channel A0 = byte 0 of each
                                // sample. TRIG:LEV is sent as voltage/attenuation;
                                // raw 0-255 maps to (raw/255)*range, so the
                                // threshold in raw units is level_scpi * 255.
                                let threshold_raw =
                                    (config.level * 255.0).clamp(0.0, 255.0) as u8;
                                Some(find_trigger_analog(
                                    &result.logic_data,
                                    us,
                                    ch_index,
                                    threshold_raw,
                                    config.edge_dir,
                                    prev_state,
                                ))
                            }
                            _ => None,
                        };

                        if let Some(scan) = scan {
                            // Persist end state for the next acquire (even if
                            // no trigger found this time — we still need the
                            // boundary state for cross-buffer detection).
                            if let Some(end) = scan.end_state {
                                *self.last_trigger_state.lock().unwrap() =
                                    Some((key, end));
                            }
                            if let Some(sample) = scan.sample {
                                result.triggered = true;
                                result.trigger_sample = sample;
                                log::info!(
                                    "Software trigger fired at sample {} (prev_state={:?})",
                                    sample, prev_state
                                );
                            }
                        }
                    }
                }
            }

            // Anchor the trigger for a stable display: keep at most `pretrigger`
            // samples before the trigger and drop the rest of the pre-trigger
            // front, so the reported buffer's trigger sits at a fixed offset
            // (0 by default = trigger at the left edge, post-trigger only). The
            // crop keeps AcquisitionData self-consistent: trigger_sample stays
            // relative to logic_data.
            if result.triggered {
                let us = result.unitsize as usize;
                if us > 0 {
                    let pre = self.pretrigger.min(result.trigger_sample);
                    let crop_bytes = ((result.trigger_sample - pre) as usize)
                        .saturating_mul(us);
                    if crop_bytes > 0 && crop_bytes <= result.logic_data.len() {
                        result.logic_data.drain(0..crop_bytes);
                    }
                    result.trigger_sample = pre;
                }
            }

            log::info!(
                "acquire: done, {} bytes logic data, unitsize={}, triggered={}, trigger_sample={}",
                result.logic_data.len(), result.unitsize, result.triggered, result.trigger_sample
            );

            Ok(result)
        }
    }
}

impl Drop for SigrokDevice {
    fn drop(&mut self) {
        unsafe {
            // Recover the Arc leaked via into_raw in open()
            if !self.cb_raw_ptr.is_null() {
                let _ = Arc::from_raw(self.cb_raw_ptr as *const CallbackState);
            }
            ffi::sr_dev_close(self.sdi);
            ffi::sr_session_destroy(self.session);
            ffi::sr_exit(self.ctx);
        }
        log::info!("Sigrok device closed");
    }
}

/// Parse a GVariant that may be:
/// - a single uint64: "t"
/// - an array of uint64: "at"
/// - a tuple of (min, max, step): "(ttt)" — expanded into discrete values
/// - a container with nested values
unsafe fn parse_gvariant_uint64_list(variant: *mut GVariant) -> Vec<u64> {
    let type_str = CStr::from_ptr(ffi::g_variant_get_type_string(variant))
        .to_str()
        .unwrap_or("");

    match type_str {
        "t" => {
            vec![ffi::g_variant_get_uint64(variant)]
        }
        "at" => {
            let n = ffi::g_variant_n_children(variant);
            let mut result = Vec::with_capacity(n);
            for i in 0..n {
                let child = ffi::g_variant_get_child_value(variant, i);
                result.push(ffi::g_variant_get_uint64(child));
                ffi::g_variant_unref(child);
            }
            result
        }
        "(ttt)" => {
            let c0 = ffi::g_variant_get_child_value(variant, 0);
            let c1 = ffi::g_variant_get_child_value(variant, 1);
            let c2 = ffi::g_variant_get_child_value(variant, 2);
            let min = ffi::g_variant_get_uint64(c0);
            let max = ffi::g_variant_get_uint64(c1);
            let step = ffi::g_variant_get_uint64(c2);
            ffi::g_variant_unref(c0);
            ffi::g_variant_unref(c1);
            ffi::g_variant_unref(c2);

            if step == 0 || min > max {
                log::warn!("Invalid (min,max,step) tuple: ({},{},{})", min, max, step);
                return vec![min];
            }

            let mut result = Vec::new();
            let mut val = min;
            while val <= max {
                result.push(val);
                val = val.saturating_add(step);
                if result.len() > 1000 {
                    log::warn!("Too many values in (min,max,step) expansion, truncating");
                    break;
                }
            }
            result
        }
        "a{sv}" => {
            let n = ffi::g_variant_n_children(variant);
            let mut result = Vec::new();
            for i in 0..n {
                let entry = ffi::g_variant_get_child_value(variant, i);
                let boxed_val = ffi::g_variant_get_child_value(entry, 1);
                let inner = ffi::g_variant_get_variant(boxed_val);
                let mut sub = parse_gvariant_uint64_list(inner);
                result.append(&mut sub);
                ffi::g_variant_unref(inner);
                ffi::g_variant_unref(boxed_val);
                ffi::g_variant_unref(entry);
            }
            result
        }
        other => {
            log::warn!("Unexpected GVariant type for config list: '{}', trying as container", other);
            let n = ffi::g_variant_n_children(variant);
            if n > 0 {
                let mut result = Vec::with_capacity(n);
                for i in 0..n {
                    let child = ffi::g_variant_get_child_value(variant, i);
                    let child_type = CStr::from_ptr(ffi::g_variant_get_type_string(child))
                        .to_str()
                        .unwrap_or("");
                    if child_type == "t" {
                        result.push(ffi::g_variant_get_uint64(child));
                    }
                    ffi::g_variant_unref(child);
                }
                result
            } else {
                Vec::new()
            }
        }
    }
}

/// The datafeed callback invoked by libsigrok during acquisition.
unsafe extern "C" fn datafeed_callback(
    _sdi: *const SrDevInst,
    packet: *const SrDatafeedPacket,
    cb_data: *mut std::ffi::c_void,
) {
    let state = &*(cb_data as *const CallbackState);

    match (*packet).type_ {
        SR_DF_HEADER => {
            log::debug!("Datafeed: SR_DF_HEADER");
        }
        SR_DF_LOGIC => {
            let logic = &*((*packet).payload as *const SrDatafeedLogic);
            let data_slice =
                std::slice::from_raw_parts(logic.data as *const u8, logic.length as usize);

            let mut acq = state.acquisition.lock().unwrap();
            acq.unitsize = logic.unitsize;
            acq.logic_data.extend_from_slice(data_slice);

            log::trace!(
                "Datafeed: SR_DF_LOGIC {} bytes, unitsize={}",
                logic.length,
                logic.unitsize
            );
        }
        SR_DF_TRIGGER => {
            let mut acq = state.acquisition.lock().unwrap();
            let samples_so_far = if acq.unitsize > 0 {
                acq.logic_data.len() as u64 / acq.unitsize as u64
            } else {
                0
            };
            acq.triggered = true;
            acq.trigger_sample = samples_so_far;
            log::debug!("Datafeed: SR_DF_TRIGGER at sample {}", samples_so_far);
        }
        SR_DF_END => {
            log::debug!("Datafeed: SR_DF_END");
        }
        other => {
            log::trace!("Datafeed: packet type {}", other);
        }
    }
}

/// Helper: convert a C string pointer to a Rust String, or "unknown" if null.
unsafe fn cstr_or_unknown(ptr: *const std::ffi::c_char) -> String {
    if ptr.is_null() {
        "unknown".to_string()
    } else {
        CStr::from_ptr(ptr)
            .to_str()
            .unwrap_or("unknown")
            .to_string()
    }
}
