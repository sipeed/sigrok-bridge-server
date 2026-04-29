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

    /// ADC mode: "digital" (default) treats each channel as a digital line;
    /// "analog" groups every 8 digital lines into one 8-bit ADC channel.
    /// Fixed at startup — client adapts on connect via CHANS? response.
    #[arg(long, default_value = "digital")]
    adc_mode: String,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    let analog_mode = match args.adc_mode.as_str() {
        "analog" | "8bit" => true,
        "digital" | "la" => false,
        other => {
            log::error!("Unknown --adc-mode '{}'. Use 'digital' or 'analog'.", other);
            std::process::exit(1);
        }
    };

    log::info!(
        "Sigrok bridge server starting: driver={}, port={}, adc_mode={}{}",
        args.driver,
        args.port,
        if analog_mode { "analog" } else { "digital" },
        args.pattern_mode.as_ref().map(|m| format!(", pattern_mode={}", m)).unwrap_or_default()
    );

    // Open device (libsigrok + glib are statically linked)
    let mut device = SigrokDevice::open(&args.driver, 0, analog_mode)
        .expect("Failed to open sigrok device");

    // Set pattern mode if specified
    if let Some(ref mode) = args.pattern_mode {
        if let Err(e) = device.set_pattern_mode(mode) {
            log::warn!("Could not set pattern mode '{}': {}", mode, e);
        }
    }

    // Query and cache available rates and depths
    device.sample_rates = device.get_available_rates().unwrap_or_else(|e| {
        log::warn!("Could not query sample rates: {}. Using defaults.", e);
        vec![1_000_000, 10_000_000, 100_000_000]
    });
    device.sample_depths = device.get_available_depths().unwrap_or_else(|e| {
        log::warn!("Could not query sample depths: {}. Using defaults.", e);
        let mut depths = Vec::new();
        let mut val = 1000u64;
        while val <= 1_000_000_000_000 {
            for &mult in &[1, 2, 5] {
                depths.push(val * mult);
            }
            val *= 10;
        }
        depths
    });

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
