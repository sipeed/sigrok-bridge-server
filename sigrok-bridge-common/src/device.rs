use crate::types::{DeviceInfo, TriggerConfig, TriggerEdgeDir};

/// Data collected from a single acquisition cycle.
pub struct AcquisitionData {
    /// Concatenated logic data buffers
    pub logic_data: Vec<u8>,
    /// Unit size (bytes per sample)
    pub unitsize: u16,
    /// Whether a trigger point was detected
    pub triggered: bool,
    /// Sample index where trigger occurred
    pub trigger_sample: u64,
}

/// Trait abstracting a bridge-compatible device (real sigrok or fake).
///
/// All methods take `&self` — implementations use interior mutability
/// (atomics, mutexes) for state that changes during operation.
pub trait BridgeDevice: Send + Sync {
    fn info(&self) -> &DeviceInfo;
    fn analog_channel_count(&self) -> u16;
    fn digital_channel_count(&self) -> u16;
    fn hw_digital_channel_count(&self) -> u16;
    fn sample_rates(&self) -> &[u64];
    fn sample_depths(&self) -> &[u64];

    fn start(&self);
    fn single(&self);
    fn stop(&self);
    fn force(&self);
    fn is_armed(&self) -> bool;
    fn is_running(&self) -> bool;
    fn is_one_shot(&self) -> bool;
    /// True when FORCE was used — acquire once and return, don't wait for trigger.
    fn is_force_triggered(&self) -> bool;
    /// Clear the force flag after a FORCE acquisition has been delivered. Called
    /// exactly once per FORCE so the next K sees normal trigger semantics again.
    fn clear_force_trigger(&self);
    fn is_shutdown(&self) -> bool;
    fn request_shutdown(&self);
    fn reset_for_new_connection(&self);
    fn next_seqnum(&self) -> u32;
    fn complete_one_shot(&self);

    fn set_rate(&self, rate: u64) -> Result<(), String>;
    fn set_depth(&self, depth: u64) -> Result<(), String>;
    fn get_rate(&self) -> u64;
    fn get_depth(&self) -> u64;

    fn set_trigger_source(&self, source: String);
    fn set_trigger_edge_dir(&self, dir: TriggerEdgeDir);
    fn set_trigger_level(&self, level: f32);
    fn set_trigger_delay(&self, delay_fs: i64);
    fn get_trigger_delay(&self) -> i64;
    fn get_trigger_config(&self) -> TriggerConfig;

    fn acquire(&self) -> Result<AcquisitionData, String>;
}
