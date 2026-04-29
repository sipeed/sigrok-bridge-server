use std::io::{self, Write};

/// Number of samples per data chunk sent over the data socket
pub const CHUNK_SAMPLES: usize = 256 * 1024;

/// Readiness byte sent by the client to request the next waveform
pub const READY_BYTE: u8 = b'K';

/// Waveform header sent over the data socket (38 bytes, little-endian).
///
/// ## Wire format
/// | Offset | Size | Type | Field         |
/// |--------|------|------|---------------|
/// | 0      | 4    | u32  | seqnum        |
/// | 4      | 2    | u16  | num_channels  |
/// | 6      | 8    | u64  | num_samples   |
/// | 14     | 8    | i64  | fs_per_sample |
/// | 22     | 8    | i64  | trigger_fs    |
/// | 30     | 8    | f64  | wfms_s        |
#[derive(Debug, Clone)]
pub struct WaveformHeader {
    /// Waveform sequence number (monotonically increasing)
    pub seqnum: u32,
    /// Number of channels in this waveform
    pub num_channels: u16,
    /// Number of samples per channel
    pub num_samples: u64,
    /// Femtoseconds per sample (1e15 / sample_rate_hz)
    pub fs_per_sample: i64,
    /// Trigger position in femtoseconds
    pub trigger_fs: i64,
    /// Hardware waveforms per second
    pub wfms_s: f64,
}

impl WaveformHeader {
    /// Total header size in bytes
    pub const SIZE: usize = 4 + 2 + 8 + 8 + 8 + 8; // 38

    /// Serialize the header to a writer in little-endian byte order
    pub fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&self.seqnum.to_le_bytes())?;
        w.write_all(&self.num_channels.to_le_bytes())?;
        w.write_all(&self.num_samples.to_le_bytes())?;
        w.write_all(&self.fs_per_sample.to_le_bytes())?;
        w.write_all(&self.trigger_fs.to_le_bytes())?;
        w.write_all(&self.wfms_s.to_le_bytes())?;
        Ok(())
    }

    /// Compute unit_size: number of bytes per sample for bit-packed data
    pub fn unit_size(&self) -> usize {
        ((self.num_channels as usize) + 7) / 8
    }
}

/// Femtoseconds per second constant (1e15)
pub const FS_PER_SECOND: i64 = 1_000_000_000_000_000;

/// Convert a sample rate in Hz to femtoseconds per sample
pub fn hz_to_fs(hz: u64) -> i64 {
    if hz == 0 {
        return 0;
    }
    FS_PER_SECOND / hz as i64
}
