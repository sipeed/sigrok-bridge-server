//! Software edge trigger detection for digital and analog waveform data.
//!
//! These functions support **cross-buffer** edge detection: callers pass in the
//! "state at the end of the previous acquisition" (`prev_state`) and receive
//! back the state at the end of the current buffer. This way, an edge that
//! straddles the gap between two captures (e.g. signal was low, the user
//! touches 3V3 during the inter-acquire idle window, next capture starts high)
//! is detected at sample 0 of the new buffer instead of being silently missed.
//!
//! Callers MUST reset `prev_state` to `None` when:
//!   - a new acquisition session starts (Start/Single/Force)
//!   - the trigger config changes (source channel, edge direction, or level)
//! Otherwise the persisted state is stale and edges may be detected or missed
//! incorrectly.

use crate::types::TriggerEdgeDir;

/// Result of a trigger scan over one acquisition buffer.
///
/// `sample` is the sample index where the matching edge was found (or None).
/// `end_state` is the logical state of the signal at the final sample of this
/// buffer, to be passed back as `prev_state` on the next call.
#[derive(Debug, Clone, Copy)]
pub struct TriggerScanResult {
    pub sample: Option<u64>,
    pub end_state: Option<bool>,
}

/// Scan bit-packed digital logic data for the first edge matching the given
/// direction on a specific channel bit, honoring cross-buffer continuity via
/// `prev_state` (`Some(true|false)` = known last bit from previous buffer,
/// `None` = no prior state, first buffer of the session).
pub fn find_trigger_digital(
    data: &[u8],
    unitsize: usize,
    channel_index: usize,
    edge_dir: TriggerEdgeDir,
    prev_state: Option<bool>,
) -> TriggerScanResult {
    if unitsize == 0 {
        return TriggerScanResult { sample: None, end_state: prev_state };
    }
    let num_samples = data.len() / unitsize;
    if num_samples == 0 {
        return TriggerScanResult { sample: None, end_state: prev_state };
    }

    let byte_offset = channel_index / 8;
    let bit_mask: u8 = 1 << (channel_index % 8);

    if byte_offset >= unitsize {
        return TriggerScanResult { sample: None, end_state: prev_state };
    }

    let get_bit = |sample: usize| -> bool {
        (data[sample * unitsize + byte_offset] & bit_mask) != 0
    };

    // Scan window depends on whether we have cross-buffer prev state.
    // With prev: start at sample 0 (compare against previous buffer's end).
    // Without: start at sample 1 (use sample 0 as initial baseline).
    let (mut prev, start_idx) = match prev_state {
        Some(s) => (s, 0),
        None => {
            if num_samples < 2 {
                return TriggerScanResult {
                    sample: None,
                    end_state: Some(get_bit(num_samples - 1)),
                };
            }
            (get_bit(0), 1)
        }
    };

    let mut found: Option<u64> = None;
    for i in start_idx..num_samples {
        let curr = get_bit(i);
        if curr != prev {
            let is_rising = curr;
            let matched = match edge_dir {
                TriggerEdgeDir::Rising => is_rising,
                TriggerEdgeDir::Falling => !is_rising,
                TriggerEdgeDir::Any => true,
            };
            if matched {
                found = Some(i as u64);
                break;
            }
            prev = curr;
        }
    }

    // end_state is the state at the last sample we actually consumed (or would
    // have consumed). For continuity across buffers this must always be the
    // last actual bit of the buffer, regardless of whether a match was found.
    let end_state = Some(get_bit(num_samples - 1));
    TriggerScanResult { sample: found, end_state }
}

/// Scan 8-bit ADC (analog) data for the first level crossing matching the
/// given direction, honoring cross-buffer continuity.
///
/// `prev_state` is the "above threshold?" boolean at the end of the previous
/// acquisition (None for the first buffer of a session).
pub fn find_trigger_analog(
    data: &[u8],
    unitsize: usize,
    channel_byte_offset: usize,
    threshold_raw: u8,
    edge_dir: TriggerEdgeDir,
    prev_state: Option<bool>,
) -> TriggerScanResult {
    if unitsize == 0 || channel_byte_offset >= unitsize {
        return TriggerScanResult { sample: None, end_state: prev_state };
    }
    let num_samples = data.len() / unitsize;
    if num_samples == 0 {
        return TriggerScanResult { sample: None, end_state: prev_state };
    }

    let get_val = |sample: usize| -> u8 {
        data[sample * unitsize + channel_byte_offset]
    };

    let (mut prev_above, start_idx) = match prev_state {
        Some(s) => (s, 0),
        None => {
            if num_samples < 2 {
                return TriggerScanResult {
                    sample: None,
                    end_state: Some(get_val(num_samples - 1) >= threshold_raw),
                };
            }
            (get_val(0) >= threshold_raw, 1)
        }
    };

    let mut found: Option<u64> = None;
    for i in start_idx..num_samples {
        let curr_above = get_val(i) >= threshold_raw;
        if curr_above != prev_above {
            let is_rising = curr_above;
            let matched = match edge_dir {
                TriggerEdgeDir::Rising => is_rising,
                TriggerEdgeDir::Falling => !is_rising,
                TriggerEdgeDir::Any => true,
            };
            if matched {
                found = Some(i as u64);
                break;
            }
            prev_above = curr_above;
        }
    }

    let end_state = Some(get_val(num_samples - 1) >= threshold_raw);
    TriggerScanResult { sample: found, end_state }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dig(data: &[u8], unitsize: usize, ch: usize, dir: TriggerEdgeDir) -> Option<u64> {
        find_trigger_digital(data, unitsize, ch, dir, None).sample
    }

    fn ana(data: &[u8], unitsize: usize, off: usize, thr: u8, dir: TriggerEdgeDir) -> Option<u64> {
        find_trigger_analog(data, unitsize, off, thr, dir, None).sample
    }

    #[test]
    fn digital_rising_edge() {
        let data: Vec<u8> = vec![0x00, 0x00, 0x01, 0x01, 0x00, 0x01];
        assert_eq!(dig(&data, 1, 0, TriggerEdgeDir::Rising), Some(2));
    }

    #[test]
    fn digital_falling_edge() {
        let data: Vec<u8> = vec![0x01, 0x01, 0x00, 0x00];
        assert_eq!(dig(&data, 1, 0, TriggerEdgeDir::Falling), Some(2));
    }

    #[test]
    fn digital_any_edge() {
        let data: Vec<u8> = vec![0x00, 0x01, 0x00];
        assert_eq!(dig(&data, 1, 0, TriggerEdgeDir::Any), Some(1));
    }

    #[test]
    fn digital_no_edge() {
        let data: Vec<u8> = vec![0x01, 0x01, 0x01];
        assert_eq!(dig(&data, 1, 0, TriggerEdgeDir::Rising), None);
    }

    #[test]
    fn digital_16ch_bit7() {
        let data: Vec<u8> = vec![0x00, 0x00, 0x80, 0x00];
        assert_eq!(dig(&data, 2, 7, TriggerEdgeDir::Rising), Some(1));
    }

    #[test]
    fn digital_16ch_bit8() {
        let data: Vec<u8> = vec![0x00, 0x00, 0x00, 0x01];
        assert_eq!(dig(&data, 2, 8, TriggerEdgeDir::Rising), Some(1));
    }

    #[test]
    fn analog_rising_crossing() {
        let data: Vec<u8> = vec![100, 110, 130, 200];
        assert_eq!(ana(&data, 1, 0, 128, TriggerEdgeDir::Rising), Some(2));
    }

    #[test]
    fn analog_falling_crossing() {
        let data: Vec<u8> = vec![200, 130, 100, 50];
        assert_eq!(ana(&data, 1, 0, 128, TriggerEdgeDir::Falling), Some(2));
    }

    #[test]
    fn empty_data() {
        assert_eq!(dig(&[], 1, 0, TriggerEdgeDir::Rising), None);
        assert_eq!(dig(&[0x00], 1, 0, TriggerEdgeDir::Rising), None);
        assert_eq!(ana(&[], 1, 0, 128, TriggerEdgeDir::Rising), None);
    }

    /// This is the cross-buffer regression test: user sees signal go low, then
    /// the rising transition happens during the idle gap between acquisitions,
    /// so the next buffer is entirely high. The trigger MUST still fire at
    /// sample 0 of the new buffer based on persisted prev_state.
    #[test]
    fn digital_rising_across_buffers() {
        // Buffer 1: all low
        let b1: Vec<u8> = vec![0x00, 0x00, 0x00, 0x00];
        let r1 = find_trigger_digital(&b1, 1, 0, TriggerEdgeDir::Rising, None);
        assert_eq!(r1.sample, None);
        assert_eq!(r1.end_state, Some(false));

        // Buffer 2: all high (transition happened between buffers)
        let b2: Vec<u8> = vec![0x01, 0x01, 0x01, 0x01];
        let r2 = find_trigger_digital(&b2, 1, 0, TriggerEdgeDir::Rising, r1.end_state);
        assert_eq!(r2.sample, Some(0), "cross-buffer rising edge must fire at sample 0");
        assert_eq!(r2.end_state, Some(true));
    }

    #[test]
    fn analog_rising_across_buffers() {
        // Buffer 1: all below threshold
        let b1: Vec<u8> = vec![10, 20, 30, 40];
        let r1 = find_trigger_analog(&b1, 1, 0, 128, TriggerEdgeDir::Rising, None);
        assert_eq!(r1.sample, None);
        assert_eq!(r1.end_state, Some(false));

        // Buffer 2: all above threshold
        let b2: Vec<u8> = vec![200, 200, 200, 200];
        let r2 = find_trigger_analog(&b2, 1, 0, 128, TriggerEdgeDir::Rising, r1.end_state);
        assert_eq!(r2.sample, Some(0), "cross-buffer analog rising must fire at sample 0");
        assert_eq!(r2.end_state, Some(true));
    }

    /// prev_state=Some(false) but buffer still low → no edge, end_state stays false.
    #[test]
    fn digital_persistent_low() {
        let data: Vec<u8> = vec![0x00, 0x00];
        let r = find_trigger_digital(&data, 1, 0, TriggerEdgeDir::Rising, Some(false));
        assert_eq!(r.sample, None);
        assert_eq!(r.end_state, Some(false));
    }

    /// prev_state=Some(true) looking for rising — should NOT false-fire on first
    /// sample (which is also high).
    #[test]
    fn digital_no_false_fire_with_high_prev() {
        let data: Vec<u8> = vec![0x01, 0x01, 0x01];
        let r = find_trigger_digital(&data, 1, 0, TriggerEdgeDir::Rising, Some(true));
        assert_eq!(r.sample, None);
        assert_eq!(r.end_state, Some(true));
    }
}
