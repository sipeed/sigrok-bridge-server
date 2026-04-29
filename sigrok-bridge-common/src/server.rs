use std::net::TcpStream;
use std::thread;
use std::time::Instant;

use crate::device::BridgeDevice;
use crate::protocol::{self, WaveformHeader};
use crate::scpi_handler::handle_scpi_commands;
use crate::twinlan::TwinLanConnection;

/// Handle a single client connection: spawn a data thread and run the SCPI loop.
pub fn handle_client<D: BridgeDevice>(mut conn: TwinLanConnection, device: &D) {
    device.reset_for_new_connection();

    let data_stream = conn
        .data_stream
        .try_clone()
        .expect("Failed to clone data stream");

    thread::scope(|s| {
        let data_handle = s.spawn(move || {
            let mut data_conn = data_stream;
            run_data_thread(&mut data_conn, device);
        });

        loop {
            match handle_scpi_commands(&mut conn, device) {
                Ok(true) => continue,
                Ok(false) => {
                    log::info!("Client disconnected (command socket EOF)");
                    break;
                }
                Err(e) => {
                    log::error!("SCPI handler error: {}", e);
                    break;
                }
            }
        }

        device.request_shutdown();
        let _ = data_handle.join();
    });
}

fn send_empty_header(data_stream: &mut TcpStream, seqnum: u32, num_channels: u16) {
    use std::io::Write;
    let header = WaveformHeader {
        seqnum,
        num_channels,
        num_samples: 0,
        fs_per_sample: 0,
        trigger_fs: 0,
        wfms_s: 0.0,
    };
    let mut buf = Vec::with_capacity(WaveformHeader::SIZE);
    let _ = header.write_to(&mut buf);
    let _ = data_stream.write_all(&buf);
    let _ = data_stream.flush();
}

fn send_waveform<D: BridgeDevice>(
    data_stream: &mut TcpStream,
    device: &D,
    acq: &crate::device::AcquisitionData,
    seqnum: u32,
    num_channels: u16,
    start: Instant,
) -> bool {
    use std::io::Write;

    let unit_size = acq.unitsize as usize;
    let num_samples = if unit_size > 0 {
        acq.logic_data.len() as u64 / unit_size as u64
    } else {
        0
    };

    let rate = device.get_rate();
    let fs_per_sample = protocol::hz_to_fs(rate);

    let trigger_fs = if acq.triggered {
        acq.trigger_sample as i64 * fs_per_sample
    } else {
        device.get_trigger_delay()
    };

    let elapsed = start.elapsed().as_secs_f64();

    let header = WaveformHeader {
        seqnum,
        num_channels,
        num_samples,
        fs_per_sample,
        trigger_fs,
        wfms_s: if elapsed > 0.0 { 1.0 / elapsed } else { 0.0 },
    };

    let mut header_buf = Vec::with_capacity(WaveformHeader::SIZE);
    if header.write_to(&mut header_buf).is_err() {
        return false;
    }
    if data_stream.write_all(&header_buf).is_err() {
        return false;
    }

    let chunk_bytes = protocol::CHUNK_SAMPLES * unit_size;
    for chunk in acq.logic_data.chunks(chunk_bytes) {
        if data_stream.write_all(chunk).is_err() {
            return false;
        }
    }

    if data_stream.flush().is_err() {
        return false;
    }

    log::debug!(
        "Sent waveform #{}: {}ch @ {} samples, triggered={}, {:.1} ms",
        seqnum, num_channels, num_samples, acq.triggered, elapsed * 1000.0
    );
    true
}

/// Data thread: receives 'K' from client, runs ONE acquisition, sends ONE response.
///
/// INVARIANT: every 'K' received produces exactly one header response.
///
/// Response semantics (header.num_samples tells the client what happened):
/// - num_samples > 0 : real waveform — either forced, or trigger fired, or no trigger source configured
/// - num_samples = 0 : "no waveform yet" — trigger did not fire / acquisition failed / state not ready
///
/// The client treats num_samples=0 as "keep previous waveform, keep trying" and will re-issue K.
/// This turns START into "continuous SINGLE" naturally, and keeps STOP responsive because the
/// client is never blocked on a long server-side wait loop.
///
/// Acquisition outcomes:
/// - FORCE or no trigger source : always deliver the captured buffer (if any)
/// - trigger-wait mode + trigger fired : deliver the waveform, complete one-shot
/// - trigger-wait mode + no trigger   : empty header, stay armed
/// - STOP mid-capture : sr_session_stop() unblocks acquire(); empty header returned
fn run_data_thread<D: BridgeDevice>(data_stream: &mut TcpStream, device: &D) {
    use std::io::Read;

    let mut ready_buf = [0u8; 1];

    loop {
        // 1. Wait for readiness byte from client
        match data_stream.read_exact(&mut ready_buf) {
            Ok(()) => {
                if ready_buf[0] != protocol::READY_BYTE {
                    log::warn!("Unexpected readiness byte: 0x{:02x}", ready_buf[0]);
                    continue;
                }
            }
            Err(e) => {
                log::debug!("Data socket read error (client disconnected?): {}", e);
                return;
            }
        }

        if device.is_shutdown() {
            return;
        }

        let num_channels = device.hw_digital_channel_count();
        let seqnum = device.next_seqnum();
        let start = Instant::now();

        // 2. If not running/armed, respond with empty header immediately so the
        //    client is never blocked. The client's own trigger-armed state will
        //    throttle how many Ks it sends in this condition.
        if !device.is_running() || !device.is_armed() {
            log::debug!(
                "Data thread: not armed (running={}, armed={}), seq={} → empty",
                device.is_running(), device.is_armed(), seqnum
            );
            send_empty_header(data_stream, seqnum, num_channels);
            continue;
        }

        // 3. Determine whether the trigger condition must fire before we send data.
        let trigger_config = device.get_trigger_config();
        let has_trigger_source = trigger_config.source_channel.is_some();
        let is_force = device.is_force_triggered();
        let skip_trigger_check = is_force || !has_trigger_source;

        log::debug!(
            "Data thread: seq={}, trigger_source={:?}, force={}, one_shot={}, skip_trigger_check={}",
            seqnum,
            trigger_config.source_channel,
            is_force,
            device.is_one_shot(),
            skip_trigger_check,
        );

        // 4. Run one acquisition. May block inside sr_session_run until hardware
        //    trigger fires or until sr_session_stop() interrupts it.
        let acq_result = device.acquire();

        let (have_data, is_triggered) = match &acq_result {
            Ok(acq) => (!acq.logic_data.is_empty(), acq.triggered),
            Err(_) => (false, false),
        };

        let send_real_waveform = have_data && (skip_trigger_check || is_triggered);

        if send_real_waveform {
            if let Ok(ref acq) = acq_result {
                log::info!(
                    "Delivering waveform seq={} (triggered={}, trig_sample={}, bytes={})",
                    seqnum, is_triggered, acq.trigger_sample, acq.logic_data.len()
                );
                if !send_waveform(data_stream, device, acq, seqnum, num_channels, start) {
                    return;
                }
            }
        } else {
            let mut backoff_ms: u64 = 0;
            match &acq_result {
                Err(e) => {
                    // Acquisition error — could be transient or persistent. Back off to
                    // prevent a client poll loop from pinning a core when the driver is
                    // misbehaving.
                    log::warn!("Acquisition error: {} (seq={})", e, seqnum);
                    backoff_ms = 100;
                }
                Ok(_) if !have_data => {
                    // No data at all (e.g. STOP interrupted acquire before any samples).
                    log::debug!("Acquisition returned no data (seq={})", seqnum);
                }
                Ok(_) => {
                    // Valid buffer but no trigger. Normal when signal hasn't met trigger
                    // condition yet. Debug-level is fine for this common case.
                    log::debug!("Trigger did not fire (seq={}); keeping armed", seqnum);
                }
            }
            send_empty_header(data_stream, seqnum, num_channels);
            if backoff_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
            }
        }

        // 5. Advance one-shot state:
        //    - FORCE / no-trigger-source: always complete (client is treating this K as one-shot too)
        //    - Trigger-wait + triggered : complete (SINGLE disarms; START stays running because one_shot=false)
        //    - Trigger-wait + not triggered : keep armed, client will send another K
        let should_complete = skip_trigger_check || is_triggered;
        if should_complete {
            device.complete_one_shot();
            // Clear force flag so the next K after a FORCE returns to normal behavior
            // (without this, FORCE during continuous mode would keep the force bit set
            // forever and the next START's trigger wait would be bypassed).
            if is_force {
                device.clear_force_trigger();
            }
        }
    }
}
