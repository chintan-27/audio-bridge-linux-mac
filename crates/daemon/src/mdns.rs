use zeroconf::{MdnsService, ServiceRegistration, ServiceDiscovery, ServiceType, TxtRecord};
use anyhow::Result;
use std::net::Ipv4Addr;

pub fn advertise_instance(instance_name: &str, port: u16) -> Result<ServiceRegistration> {
    let ty = ServiceType::new("_audiobridge._udp")?;
    let host_ipv4 = Ipv4Addr::UNSPECIFIED;
    let mut service = MdnsService::new(ty, instance_name, host_ipv4, port);
    let mut txt = TxtRecord::new();
    txt.insert("codec", "opus")?;
    txt.insert("clock", "48000")?;
    service.set_txt_record(txt);

    Ok(service.register()?)
}

// Discovery example (blocking, minimal)
pub fn discover_once() -> Result<()> {
    let ty = ServiceType::new("_audiobridge._udp")?;
    let discovery = ServiceDiscovery::browse(ty)?;
    for event in discovery {
        // Handle events (add/remove); wire to your UI or logs
        println!("mDNS event: {:?}", event);
    }
    Ok(())
}
