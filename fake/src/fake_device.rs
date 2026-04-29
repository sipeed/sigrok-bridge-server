use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use sigrok_bridge_common::device::{AcquisitionData, BridgeDevice};
use sigrok_bridge_common::trigger::find_trigger_digital;
use sigrok_bridge_common::types::{parse_channel_hwname, DeviceInfo, TriggerConfig, TriggerEdgeDir};

use crate::waveform_gen;

#[derive(Debug, Clone, PartialEq)]
struct TriggerStateKey {
    source: Option<String>,
    edge_dir: TriggerEdgeDir,
    level: f32,
}

/// Fake device state machine simulating a sigrok-compatible logic analyzer/oscilloscope
pub struct FakeDevice {
    info: DeviceInfo,
    analog_channel_count: u16,
    digital_channel_count: u16,
    sample_rates: Vec<u64>,
    sample_depths: Vec<u64>,

    current_rate: Mutex<u64>,
    current_depth: Mutex<u64>,

    running: AtomicBool,
    one_shot: AtomicBool,
    force_trigger: AtomicBool,
    trigger_armed: AtomicBool,
    shutdown: AtomicBool,
    seqnum: AtomicU32,
    trigger_config: Mutex<TriggerConfig>,
    last_trigger_state: Mutex<Option<(TriggerStateKey, bool)>>,
}

impl FakeDevice {
    /// Create a new fake device with 16 digital channels (pure logic analyzer)
    pub fn new_la16() -> Self {
        let rates = vec![
            1_000_000,    // 1 MHz
            2_000_000,    // 2 MHz
            5_000_000,    // 5 MHz
            10_000_000,   // 10 MHz
            20_000_000,   // 20 MHz
            50_000_000,   // 50 MHz
            100_000_000,  // 100 MHz
            200_000_000,  // 200 MHz
        ];
        let depths = vec![
            1_000,
            10_000,
            100_000,
            1_000_000,
            10_000_000,
        ];

        Self {
            info: DeviceInfo {
                vendor: "FakeBridge".to_string(),
                model: "SLogic16U3-Fake".to_string(),
                serial: "FAKE001".to_string(),
                firmware: "1.0".to_string(),
            },
            analog_channel_count: 0,
            digital_channel_count: 16,
            sample_rates: rates.clone(),
            sample_depths: depths.clone(),
            current_rate: Mutex::new(rates[0]),
            current_depth: Mutex::new(depths[0]),
            running: AtomicBool::new(false),
            one_shot: AtomicBool::new(false),
            force_trigger: AtomicBool::new(false),
            trigger_armed: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            seqnum: AtomicU32::new(0),
            trigger_config: Mutex::new(TriggerConfig::default()),
            last_trigger_state: Mutex::new(None),
        }
    }
}

impl BridgeDevice for FakeDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn analog_channel_count(&self) -> u16 {
        self.analog_channel_count
    }

    fn digital_channel_count(&self) -> u16 {
        self.digital_channel_count
    }

    fn hw_digital_channel_count(&self) -> u16 {
        self.digital_channel_count
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

    fn is_armed(&self) -> bool {
        self.trigger_armed.load(Ordering::SeqCst)
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

    fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
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
        *self.current_rate.lock().unwrap() = rate;
        log::info!("Sample rate set to {} Hz", rate);
        Ok(())
    }

    fn set_depth(&self, depth: u64) -> Result<(), String> {
        *self.current_depth.lock().unwrap() = depth;
        log::info!("Sample depth set to {} samples", depth);
        Ok(())
    }

    fn get_rate(&self) -> u64 {
        *self.current_rate.lock().unwrap()
    }

    fn get_depth(&self) -> u64 {
        *self.current_depth.lock().unwrap()
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

    fn acquire(&self) -> Result<AcquisitionData, String> {
        let num_channels = self.digital_channel_count;
        let depth = self.get_depth();

        let data = waveform_gen::generate_waveform_data(num_channels, depth);
        let unit_size = ((num_channels as usize) + 7) / 8;

        let config = self.get_trigger_config();
        let (triggered, trigger_sample) = if let Some(ref source) = config.source_channel {
            if let Some((ch_type, ch_index)) = parse_channel_hwname(source) {
                let key = TriggerStateKey {
                    source: Some(source.clone()),
                    edge_dir: config.edge_dir,
                    level: config.level,
                };
                let prev_state: Option<bool> = {
                    let guard = self.last_trigger_state.lock().unwrap();
                    match &*guard {
                        Some((k, s)) if *k == key => Some(*s),
                        _ => None,
                    }
                };

                let scan = match ch_type {
                    'D' | 'd' => Some(find_trigger_digital(
                        &data, unit_size, ch_index, config.edge_dir, prev_state,
                    )),
                    _ => None,
                };

                if let Some(scan) = scan {
                    if let Some(end) = scan.end_state {
                        *self.last_trigger_state.lock().unwrap() = Some((key, end));
                    }
                    match scan.sample {
                        Some(s) => (true, s),
                        None => (false, 0),
                    }
                } else {
                    (false, 0)
                }
            } else {
                (false, 0)
            }
        } else {
            (false, 0)
        };

        Ok(AcquisitionData {
            logic_data: data,
            unitsize: unit_size as u16,
            triggered,
            trigger_sample,
        })
    }
}
