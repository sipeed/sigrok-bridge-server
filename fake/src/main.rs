mod fake_device;
mod waveform_gen;

use sigrok_bridge_common::server;
use sigrok_bridge_common::twinlan::TwinLanServer;

use fake_device::FakeDevice;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10101);

    log::info!("Fake sigrok bridge server starting on port {}", port);

    let device = FakeDevice::new_la16();

    let server_sock = TwinLanServer::bind(port).expect("Failed to bind server");

    loop {
        log::info!("Waiting for client...");
        let conn = match server_sock.accept() {
            Ok(c) => c,
            Err(e) => {
                log::error!("Accept error: {}", e);
                continue;
            }
        };
        log::info!("Client connected");

        server::handle_client(conn, &device);

        log::info!("Client disconnected");
    }
}
