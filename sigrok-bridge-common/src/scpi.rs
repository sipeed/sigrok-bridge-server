/// Parsed SCPI command from the client
#[derive(Debug)]
pub enum ScpiCommand {
    // Identification
    Idn,

    // Channel queries
    Chans,
    Layout,
    Rates,
    Depths,

    // Acquisition control
    Start,
    Single,
    Stop,
    Force,
    Armed,

    // Sample configuration
    Depth(u64),
    Rate(u64),

    // Channel configuration (channel hwname, e.g. "A0", "D3")
    ChannelOn(String),
    ChannelOff(String),
    ChannelCoupling(String, String),
    ChannelRange(String, f32),
    ChannelOffset(String, f32),
    ChannelThreshold(String, f32),

    // Trigger configuration
    TrigDelay(i64),
    TrigSource(String),
    TrigLevel(f32),
    TrigEdgeDir(String),
}

/// Parse a single SCPI command line into a ScpiCommand.
///
/// Returns None if the command is unrecognized.
pub fn parse_scpi_line(line: &str) -> Option<ScpiCommand> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    // Simple commands (no arguments)
    match line {
        "*IDN?" => return Some(ScpiCommand::Idn),
        "CHANS?" => return Some(ScpiCommand::Chans),
        "LAYOUT?" => return Some(ScpiCommand::Layout),
        "RATES?" => return Some(ScpiCommand::Rates),
        "DEPTHS?" => return Some(ScpiCommand::Depths),
        "START" => return Some(ScpiCommand::Start),
        "SINGLE" => return Some(ScpiCommand::Single),
        "STOP" => return Some(ScpiCommand::Stop),
        "FORCE" => return Some(ScpiCommand::Force),
        "ARMED?" => return Some(ScpiCommand::Armed),
        _ => {}
    }

    // Commands with arguments
    if let Some(val) = line.strip_prefix("DEPTH ") {
        return val.trim().parse::<u64>().ok().map(ScpiCommand::Depth);
    }
    if let Some(val) = line.strip_prefix("RATE ") {
        return val.trim().parse::<u64>().ok().map(ScpiCommand::Rate);
    }

    // Trigger commands
    if let Some(val) = line.strip_prefix("TRIG:DELAY ") {
        return val.trim().parse::<i64>().ok().map(ScpiCommand::TrigDelay);
    }
    if let Some(val) = line.strip_prefix("TRIG:SOU ") {
        return Some(ScpiCommand::TrigSource(val.trim().to_string()));
    }
    if let Some(val) = line.strip_prefix("TRIG:LEV ") {
        return val.trim().parse::<f32>().ok().map(ScpiCommand::TrigLevel);
    }
    if let Some(val) = line.strip_prefix("TRIG:EDGE:DIR ") {
        return Some(ScpiCommand::TrigEdgeDir(val.trim().to_string()));
    }

    // Channel commands: :HWNAME:SUBCOMMAND [args]
    // Format: ":A0:ON", ":D3:COUP DC1M", ":A1:RANGE 5.0", etc.
    if line.starts_with(':') {
        let rest = &line[1..]; // skip leading ':'
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            let hwname = parts[0].to_string();
            let subcmd = parts[1];

            if subcmd == "ON" {
                return Some(ScpiCommand::ChannelOn(hwname));
            }
            if subcmd == "OFF" {
                return Some(ScpiCommand::ChannelOff(hwname));
            }
            if let Some(val) = subcmd.strip_prefix("COUP ") {
                return Some(ScpiCommand::ChannelCoupling(hwname, val.trim().to_string()));
            }
            if let Some(val) = subcmd.strip_prefix("RANGE ") {
                return val
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|v| ScpiCommand::ChannelRange(hwname, v));
            }
            if let Some(val) = subcmd.strip_prefix("OFFS ") {
                return val
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|v| ScpiCommand::ChannelOffset(hwname, v));
            }
            if let Some(val) = subcmd.strip_prefix("THRESH ") {
                return val
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|v| ScpiCommand::ChannelThreshold(hwname, v));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_commands() {
        assert!(matches!(parse_scpi_line("*IDN?"), Some(ScpiCommand::Idn)));
        assert!(matches!(parse_scpi_line("CHANS?"), Some(ScpiCommand::Chans)));
        assert!(matches!(parse_scpi_line("START"), Some(ScpiCommand::Start)));
        assert!(matches!(parse_scpi_line("STOP"), Some(ScpiCommand::Stop)));
        assert!(matches!(parse_scpi_line("ARMED?"), Some(ScpiCommand::Armed)));
    }

    #[test]
    fn test_parse_with_args() {
        assert!(matches!(parse_scpi_line("DEPTH 10000"), Some(ScpiCommand::Depth(10000))));
        assert!(matches!(parse_scpi_line("RATE 200000000"), Some(ScpiCommand::Rate(200000000))));
    }

    #[test]
    fn test_parse_channel_commands() {
        assert!(matches!(parse_scpi_line(":A0:ON"), Some(ScpiCommand::ChannelOn(ref n)) if n == "A0"));
        assert!(matches!(parse_scpi_line(":D3:OFF"), Some(ScpiCommand::ChannelOff(ref n)) if n == "D3"));
        assert!(matches!(parse_scpi_line(":A0:COUP DC1M"), Some(ScpiCommand::ChannelCoupling(ref n, ref c)) if n == "A0" && c == "DC1M"));
        assert!(matches!(parse_scpi_line(":D0:THRESH 1.6"), Some(ScpiCommand::ChannelThreshold(ref n, v)) if n == "D0" && (v - 1.6).abs() < 0.01));
    }

    #[test]
    fn test_parse_layout_query() {
        assert!(matches!(parse_scpi_line("LAYOUT?"), Some(ScpiCommand::Layout)));
    }

    #[test]
    fn test_parse_trigger_commands() {
        assert!(matches!(parse_scpi_line("TRIG:DELAY 1000000000000"), Some(ScpiCommand::TrigDelay(1000000000000))));
        assert!(matches!(parse_scpi_line("TRIG:SOU A0"), Some(ScpiCommand::TrigSource(ref s)) if s == "A0"));
        assert!(matches!(parse_scpi_line("TRIG:EDGE:DIR RISING"), Some(ScpiCommand::TrigEdgeDir(ref d)) if d == "RISING"));
    }
}
