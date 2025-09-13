use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name="ab-daemon", version, about="Rust LAN audio bridge")]
pub struct Args {
    /// Optional device name to capture (e.g., "BlackHole 2ch" on macOS)
    #[arg(long)]
    pub capture_device: Option<String>,

    /// Remote host to send to (IPv4 LAN)
    #[arg(long)]
    pub send_to: Option<String>,

    /// Send port
    #[arg(long, default_value_t = 5002)]
    pub send_port: u16,

    /// Listen port
    #[arg(long, default_value_t = 5004)]
    pub listen_port: u16,

    /// Advertise & discover peers on mDNS
    #[arg(long, default_value_t = true)]
    pub mdns: bool,
}
