use crate::device::BridgeDevice;
use crate::scpi::{parse_scpi_line, ScpiCommand};
use crate::twinlan::TwinLanConnection;
use crate::types::TriggerEdgeDir;

/// Handle SCPI commands from the command socket, dispatching to a BridgeDevice.
///
/// Returns Ok(true) to continue, Ok(false) on client disconnect, Err on I/O error.
pub fn handle_scpi_commands<D: BridgeDevice>(
    conn: &mut TwinLanConnection,
    device: &D,
) -> std::io::Result<bool> {
    let line = match conn.read_command()? {
        Some(l) => l,
        None => return Ok(false),
    };

    if line.is_empty() {
        return Ok(true);
    }

    log::debug!("SCPI recv: {}", line);

    let cmd = match parse_scpi_line(&line) {
        Some(c) => c,
        None => {
            // parse_scpi_line already logs a warning for unrecognized commands
            return Ok(true);
        }
    };

    match cmd {
        // Queries
        ScpiCommand::Idn => {
            conn.send_reply(&device.info().to_idn_string())?;
        }
        ScpiCommand::Chans => {
            conn.send_reply(&format!(
                "{},{}",
                device.analog_channel_count(),
                device.digital_channel_count()
            ))?;
        }
        ScpiCommand::Rates => {
            let rates: Vec<String> = device.sample_rates().iter().map(|r| r.to_string()).collect();
            conn.send_reply(&rates.join(","))?;
        }
        ScpiCommand::Depths => {
            let depths: Vec<String> =
                device.sample_depths().iter().map(|d| d.to_string()).collect();
            conn.send_reply(&depths.join(","))?;
        }
        ScpiCommand::Armed => {
            conn.send_reply(if device.is_armed() { "1" } else { "0" })?;
        }

        // Acquisition control
        ScpiCommand::Start => device.start(),
        ScpiCommand::Single => device.single(),
        ScpiCommand::Stop => device.stop(),
        ScpiCommand::Force => device.force(),

        // Configuration
        ScpiCommand::Depth(d) => {
            if let Err(e) = device.set_depth(d) {
                log::error!("Failed to set depth: {}", e);
            }
        }
        ScpiCommand::Rate(r) => {
            if let Err(e) = device.set_rate(r) {
                log::error!(
                    "Failed to set rate: {}. Active rate remains {} Hz",
                    e,
                    device.get_rate()
                );
            }
        }

        // Channel commands (log only — hardware channels are physical)
        ScpiCommand::ChannelOn(ch) => log::debug!("Channel {} enabled", ch),
        ScpiCommand::ChannelOff(ch) => log::debug!("Channel {} disabled", ch),
        ScpiCommand::ChannelCoupling(ch, c) => log::debug!("Channel {} coupling: {}", ch, c),
        ScpiCommand::ChannelRange(ch, r) => log::debug!("Channel {} range: {}", ch, r),
        ScpiCommand::ChannelOffset(ch, o) => log::debug!("Channel {} offset: {}", ch, o),
        ScpiCommand::ChannelThreshold(ch, t) => log::debug!("Channel {} threshold: {}", ch, t),

        // Trigger configuration
        ScpiCommand::TrigDelay(d) => device.set_trigger_delay(d),
        ScpiCommand::TrigSource(s) => device.set_trigger_source(s),
        ScpiCommand::TrigLevel(l) => device.set_trigger_level(l),
        ScpiCommand::TrigEdgeDir(d) => match TriggerEdgeDir::from_scpi(&d) {
            Some(dir) => device.set_trigger_edge_dir(dir),
            None => log::warn!("Unknown trigger edge direction: {}", d),
        },

        // ADC mode (fixed at bridge startup)
        ScpiCommand::AdcMode(m) => log::debug!("ADC mode set to {}", m),
    }

    Ok(true)
}
