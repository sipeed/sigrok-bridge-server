use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

/// A twinlan server that manages two TCP sockets:
/// - Command socket (primary): line-based SCPI text protocol
/// - Data socket (secondary): binary waveform data protocol
pub struct TwinLanServer {
    cmd_listener: TcpListener,
    data_listener: TcpListener,
}

/// An accepted twinlan client connection pair
pub struct TwinLanConnection {
    pub cmd_reader: BufReader<TcpStream>,
    pub cmd_writer: TcpStream,
    pub data_stream: TcpStream,
}

impl TwinLanServer {
    /// Create a new TwinLanServer binding to the given command port.
    /// The data port is command_port + 1 (matching SCPITwinLanTransport convention).
    pub fn bind(cmd_port: u16) -> io::Result<Self> {
        let data_port = cmd_port + 1;
        let cmd_listener = TcpListener::bind(("0.0.0.0", cmd_port))?;
        let data_listener = TcpListener::bind(("0.0.0.0", data_port))?;

        log::info!("Command socket listening on port {}", cmd_port);
        log::info!("Data socket listening on port {}", data_port);

        Ok(Self {
            cmd_listener,
            data_listener,
        })
    }

    /// Accept a client connection on both sockets.
    /// Blocks until both command and data connections are established.
    pub fn accept(&self) -> io::Result<TwinLanConnection> {
        let cmd_port = self.cmd_listener.local_addr()?.port();
        let data_port = self.data_listener.local_addr()?.port();

        log::info!("Waiting for command connection on port {}...", cmd_port);
        let (cmd_stream, cmd_addr) = self.cmd_listener.accept()?;
        log::info!("Command connection from {}", cmd_addr);

        // Disable Nagle on command socket for responsive SCPI
        cmd_stream.set_nodelay(true)?;

        log::info!("Waiting for data connection on port {}...", data_port);
        let (data_stream, data_addr) = self.data_listener.accept()?;
        log::info!("Data connection from {}", data_addr);

        // Disable Nagle on data socket for low-latency data transfer
        data_stream.set_nodelay(true)?;

        let cmd_writer = cmd_stream.try_clone()?;
        let cmd_reader = BufReader::new(cmd_stream);

        Ok(TwinLanConnection {
            cmd_reader,
            cmd_writer,
            data_stream,
        })
    }
}

impl TwinLanConnection {
    /// Read a single SCPI command line from the command socket.
    /// Returns None on EOF (client disconnected).
    pub fn read_command(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        let n = self.cmd_reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        // Trim trailing newline/carriage return
        let trimmed = line.trim_end().to_string();
        Ok(Some(trimmed))
    }

    /// Send a reply on the command socket (appends newline)
    pub fn send_reply(&mut self, reply: &str) -> io::Result<()> {
        write!(self.cmd_writer, "{}\n", reply)?;
        self.cmd_writer.flush()
    }
}
