mod sigrok_device;

use std::sync::Arc;

use clap::Parser;

use sigrok_bridge_common::device::BridgeDevice;
use sigrok_bridge_common::server;
use sigrok_bridge_common::twinlan::TwinLanServer;

use sigrok_device::SigrokDevice;

#[derive(Parser)]
#[command(about = "Sigrok bridge server for ngscopeclient")]
struct Args {
    /// Sigrok driver name
    #[arg(short, long, default_value = "sipeed-slogic-analyzer")]
    driver: String,

    /// TCP port for twinlan transport
    #[arg(short, long, default_value_t = 10101)]
    port: u16,

    /// Pattern mode: Normal, "USB connection test", Emulation
    #[arg(long)]
    pattern_mode: Option<String>,

    /// Physical capture width in channels (8, 16, or 32). Sets
    /// SR_CONF_NUM_LOGIC_CHANNELS on the device so the driver's rate ceiling
    /// follows. Default: the device's hardware maximum.
    #[arg(long)]
    max_channels: Option<u32>,

    /// Comma-separated byte-groups to expose as 8-bit analog channels, named
    /// `A<idx>` (e.g. "A0,A2,A3"). Each `idx` is a byte-group index into the
    /// capture width; all other groups in 0..max_channels/8 are digital.
    /// Default: none (every group digital).
    #[arg(long)]
    analog_groups: Option<String>,

    /// Pre-trigger samples to keep before the trigger point. Default 0: the
    /// trigger edge is the left edge of the capture (post-trigger only, so a
    /// zoomed view naturally keeps t=0 at the left and pre-trigger is never
    /// shown). Set >0 to retain that many samples before the trigger (they sit
    /// at t<0, left of the trigger; pan left to see them).
    #[arg(long, default_value_t = 0)]
    pretrigger: u64,
}

/// Parse an `--analog-groups` spec like "A0,A2,A3" into byte-group indices.
/// Tokens are trimmed; the leading `A`/`a` prefix is optional. Invalid tokens
/// are logged and skipped. Group indices out of range for the chosen
/// max_channels are filtered later (in `SigrokDevice::open`) with a warning.
fn parse_analog_groups(spec: &str) -> Vec<usize> {
    spec.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .filter_map(|tok| {
            let digits = tok.strip_prefix(['A', 'a']).unwrap_or(tok);
            match digits.parse::<usize>() {
                Ok(idx) => Some(idx),
                Err(_) => {
                    log::warn!("Ignoring invalid analog group '{}'", tok);
                    None
                }
            }
        })
        .collect()
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    if let Some(mc) = args.max_channels {
        if !matches!(mc, 8 | 16 | 32) {
            log::error!("Invalid --max-channels '{}'. Use 8, 16, or 32.", mc);
            std::process::exit(1);
        }
    }

    let analog_groups = args
        .analog_groups
        .as_deref()
        .map(parse_analog_groups)
        .unwrap_or_default();

    log::info!(
        "Sigrok bridge server starting: driver={}, port={}, max_channels={}, analog_groups={:?}, pretrigger={}{}",
        args.driver,
        args.port,
        args.max_channels.map(|m| m.to_string()).unwrap_or_else(|| "device-max".into()),
        analog_groups,
        args.pretrigger,
        args.pattern_mode.as_ref().map(|m| format!(", pattern_mode={}", m)).unwrap_or_default()
    );

    // Open device (libsigrok + glib are statically linked)
    let mut device =
        SigrokDevice::open(&args.driver, 0, args.max_channels, &analog_groups, args.pretrigger)
            .expect("Failed to open sigrok device");

    // Set pattern mode if specified
    if let Some(ref mode) = args.pattern_mode {
        if let Err(e) = device.set_pattern_mode(mode) {
            log::warn!("Could not set pattern mode '{}': {}", mode, e);
        }
    }

    // Query and cache available rates and depths
    device.sample_rates = device
        .get_available_rates()
        .expect("Device driver did not report sample rates");
    device.sample_depths = device
        .get_available_depths()
        .expect("Device driver did not report sample depths");

    // Set initial rate and depth
    if let Some(&rate) = device.sample_rates.first() {
        let _ = device.set_rate(rate);
    }
    if let Some(&depth) = device.sample_depths.first() {
        let _ = device.set_depth(depth);
    }

    let device = Arc::new(device);

    // Start TCP server
    let server_sock = TwinLanServer::bind(args.port).expect("Failed to bind server");

    loop {
        log::info!("Waiting for client on port {}...", args.port);
        let conn = match server_sock.accept() {
            Ok(c) => c,
            Err(e) => {
                log::error!("Accept error: {}", e);
                continue;
            }
        };
        log::info!("Client connected");

        server::handle_client(conn, &*device);

        log::info!("Client disconnected");
    }
}
