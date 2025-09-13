use anyhow::Result;
use clap::Parser;
use ab_core::pipeline::{init_gst, build_receiver, build_sender};
mod args;
mod mdns;

#[tokio::main]
async fn main() -> Result<()> {
    let a = args::Args::parse();
    init_gst()?;

    // Receiver always on (so the other side can send anytime)
    let rx = build_receiver(a.listen_port)?;
    rx.start()?;

    // Optional: advertise listen_port for others
    let _reg = if a.mdns {
        Some(mdns::advertise_instance("ab-node", a.listen_port)?)
    } else { None };

    // Optional sender if send_to provided
    let _tx = if let Some(host) = a.send_to.as_deref() {
        let tx = build_sender(a.capture_device.as_deref(), host, a.send_port)?;
        tx.start()?;
        Some(tx)
    } else { None };

    // Keep running
    tokio::signal::ctrl_c().await?;
    Ok(())
}
