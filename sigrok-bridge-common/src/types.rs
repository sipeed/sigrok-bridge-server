/// One 8-bit byte-group of the physical capture width, tagged as either analog
/// (the whole byte is one ADC channel) or digital (the byte's 8 bits are 8
/// logic lines). The ordered set of these — one per byte, ascending
/// `byte_offset` — is the channel layout advertised to the client via LAYOUT?.
#[derive(Clone, Debug)]
pub struct ChannelGroup {
    pub analog: bool,
    pub byte_offset: usize,
    pub name: String,
}

/// Device information returned by *IDN? query
pub struct DeviceInfo {
    pub vendor: String,
    pub model: String,
    pub serial: String,
    pub firmware: String,
}

impl DeviceInfo {
    pub fn to_idn_string(&self) -> String {
        format!("{},{},{},{}", self.vendor, self.model, self.serial, self.firmware)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TriggerEdgeDir {
    Rising,
    Falling,
    Any,
}

impl TriggerEdgeDir {
    pub fn from_scpi(s: &str) -> Option<Self> {
        match s {
            "RISING" => Some(Self::Rising),
            "FALLING" => Some(Self::Falling),
            "ANY" => Some(Self::Any),
            _ => None,
        }
    }
}

/// Stored trigger configuration, updated by SCPI TRIG:* commands.
#[derive(Debug, Clone)]
pub struct TriggerConfig {
    /// Trigger source channel hardware name (e.g. "D0", "A1").
    /// None means no trigger source configured yet.
    pub source_channel: Option<String>,
    /// Edge direction
    pub edge_dir: TriggerEdgeDir,
    /// Trigger level (analog threshold, already divided by attenuation on the C++ side)
    pub level: f32,
    /// Trigger delay/offset in femtoseconds (sent by TRIG:DELAY)
    pub delay_fs: i64,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            source_channel: None,
            edge_dir: TriggerEdgeDir::Rising,
            level: 0.0,
            delay_fs: 1_000_000_000_000, // 1ms default, matching C++ constructor
        }
    }
}

/// Parse a channel hardware name from SCPI (e.g. "D0", "A1") into a (type_prefix, index) pair.
pub fn parse_channel_hwname(hwname: &str) -> Option<(char, usize)> {
    let mut chars = hwname.chars();
    let prefix = chars.next()?;
    if !prefix.is_ascii_alphabetic() {
        return None;
    }
    let index_str: String = chars.collect();
    let index: usize = index_str.parse().ok()?;
    Some((prefix, index))
}
