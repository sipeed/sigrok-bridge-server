/// Generate fake waveform data (bit-packed).
///
/// The same data is used for both Digital and Analog modes:
/// - Digital mode: 16 channels, each bit is a separate digital channel
/// - Analog mode: D0-D7 form one 8-bit ADC value (A0), D8-D15 form another (A1)
///
/// D0 has the highest frequency, each subsequent channel halves (binary divider).
/// D0 toggles every sample, D1 every 2, D2 every 4, ..., D15 every 32768.
/// This is the classic binary counter pattern (sample index bits).
///
/// Returns a Vec<u8> of `num_samples * unit_size` bytes.
pub fn generate_waveform_data(num_channels: u16, num_samples: u64) -> Vec<u8> {
    let unit_size = ((num_channels as usize) + 7) / 8;
    let total_bytes = num_samples as usize * unit_size;
    let mut data = vec![0u8; total_bytes];

    for sample_idx in 0..num_samples as usize {
        // Binary counter: bit N of sample_idx naturally gives a square wave
        // that halves in frequency for each higher bit — D0 fastest, D15 slowest.
        for byte in 0..unit_size {
            let val = (sample_idx >> (byte * 8)) as u8;
            data[sample_idx * unit_size + byte] = val;
        }
    }

    data
}
