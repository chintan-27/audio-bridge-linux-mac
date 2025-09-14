use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::env;

/* ------------------------------------------------------------------------- */
/* Types                                                                      */
/* ------------------------------------------------------------------------- */

pub struct Sender {
    pipeline: gst::Pipeline,
}
pub struct Receiver {
    pipeline: gst::Pipeline,
}

/* ------------------------------------------------------------------------- */
/* Utilities & logging                                                        */
/* ------------------------------------------------------------------------- */

fn make_element(factory: &str, name: &str) -> Result<gst::Element> {
    let e = gst::ElementFactory::make(factory)
        .name(name)
        .build()
        .with_context(|| format!("failed to make element '{factory}' as '{name}'"))?;
    eprintln!("[build] created {factory} as {name}");
    Ok(e)
}

fn attach_bus_logging(p: &gst::Pipeline, tag: &str) {
    if let Some(bus) = p.bus() {
        let tag = tag.to_string();
        std::thread::spawn(move || {
            for msg in bus.iter_timed(gst::ClockTime::NONE) {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Error(e) => eprintln!(
                        "[{tag}] ERROR from {}: {} (debug: {:?})",
                        e.src()
                            .map(|s| s.path_string())
                            .unwrap_or_else(|| "<unknown>".into()),
                        e.error(),
                        e.debug()
                    ),
                    MessageView::Warning(w) => eprintln!(
                        "[{tag}] WARN  from {}: {} (debug: {:?})",
                        w.src()
                            .map(|s| s.path_string())
                            .unwrap_or_else(|| "<unknown>".into()),
                        w.error(),
                        w.debug()
                    ),
                    MessageView::Info(i) => eprintln!(
                        "[{tag}] INFO  from {}: {} (debug: {:?})",
                        i.src()
                            .map(|s| s.path_string())
                            .unwrap_or_else(|| "<unknown>".into()),
                        i.error(),
                        i.debug()
                    ),
                    MessageView::Element(el) => {
                        if let Some(s) = el.structure() {
                            eprintln!("[{tag}] ELEMENT {}", s.to_string());
                        }
                    }
                    MessageView::StateChanged(s) => {
                        if let Some(src) = msg.src() {
                            if src.type_().is_a(gst::Pipeline::static_type()) {
                                eprintln!(
                                    "[{tag}] state changed: {:?} -> {:?} (pending {:?})",
                                    s.old(),
                                    s.current(),
                                    s.pending()
                                );
                            }
                        }
                    }
                    MessageView::Latency(_) => eprintln!("[{tag}] latency message"),
                    _ => {}
                }
            }
        });
    }
}

fn attach_caps_probe(elem: &gst::Element, pad_name: &str, tag: &str) {
    if let Some(pad) = elem.static_pad(pad_name) {
        let t = tag.to_string();
        let p = pad_name.to_string();
        pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_pad, info| {
            if let Some(ev) = info.event() {
                if let gst::EventView::Caps(c) = ev.view() {
                    eprintln!("[caps:{t}] {p} -> {}", c.caps().to_string());
                }
            }
            gst::PadProbeReturn::Ok
        });
    }
}

/// Attach a simple TX stats probe that counts packets/bytes and logs ~1s.
fn attach_tx_stats(elem: &gst::Element, pad_name: &str, tag: &str) {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    if let Some(pad) = elem.static_pad(pad_name) {
        let state = Arc::new(Mutex::new((0u64, 0u64, Instant::now())));
        let t = tag.to_string();
        let st = state.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
            if let Some(buf) = info.buffer() {
                let sz = buf.size() as u64;
                let mut s = st.lock().unwrap();
                s.0 += 1;
                s.1 += sz;
                let dt = s.2.elapsed();
                if dt >= Duration::from_secs(1) {
                    let pkts = s.0;
                    let bytes = s.1;
                    let bps = (bytes as f64) * 8.0 / dt.as_secs_f64();
                    let kbps = bps / 1000.0;
                    eprintln!(
                        "[{t}] TX ~{:.0} pkts/s, ~{:.1} kbit/s ({} bytes in {:.2}s)",
                        pkts as f64 / dt.as_secs_f64(),
                        kbps,
                        bytes,
                        dt.as_secs_f64()
                    );
                    *s = (0, 0, Instant::now());
                }
            }
            gst::PadProbeReturn::Ok
        });
    }
}

pub fn init_gst() -> Result<()> {
    gst::init().context("gst init failed")?;
    eprintln!(
        "[init] GStreamer {} (GST_PLUGIN_PATH={:?})",
        gst::version_string(),
        env::var("GST_PLUGIN_PATH").ok()
    );
    Ok(())
}

/* ------------------------------------------------------------------------- */
/* Linux helper: pick a PulseAudio/pipewire *monitor* source                  */
/* ------------------------------------------------------------------------- */

#[cfg(target_os = "linux")]
fn pick_pulse_monitor(prefer_contains: Option<&str>) -> Option<String> {
    use gst::{caps::Caps, DeviceMonitor};
    let mon = DeviceMonitor::new();
    mon.add_filter(Some("Audio/Source"), Some(&Caps::new_any()));
    mon.start().ok()?;

    let hint = prefer_contains.map(|s| s.to_lowercase());
    let mut first_monitor: Option<String> = None;
    let mut preferred: Option<String> = None;

    for d in mon.devices() {
        let name = d.display_name().to_string();     // FIX: GString -> String
        let class = d.device_class().to_string();    // FIX: GString -> String
        let props = d.properties();

        // Consider anything whose display name or known props contain "monitor"
        let mut is_monitor = name.to_lowercase().contains("monitor");
        let mut dev_id: Option<String> = None;

        if let Some(p) = props {
            // Pulsesrc expects the "device" string (e.g., "alsa_output...analog-stereo.monitor")
            if let Ok(v) = p.get::<String>("device") {
                dev_id = Some(v);
            } else if let Ok(v) = p.get::<String>("node.name") {
                dev_id = Some(v);
            }

            // Extra checks for monitor-ness
            if !is_monitor {
                if let Ok(desc) = p.get::<String>("device.description") {
                    if desc.to_lowercase().contains("monitor") {
                        is_monitor = true;
                    }
                }
                if let Ok(nodename) = p.get::<String>("node.name") {
                    if nodename.to_lowercase().contains("monitor") {
                        is_monitor = true;
                    }
                }
            }
        }

        if is_monitor {
            let id = dev_id.clone().unwrap_or_else(|| name.clone());
            eprintln!(
                "[linux] found monitor candidate: id='{}' name='{}' class='{}'",
                id, name, class
            );

            if first_monitor.is_none() {
                first_monitor = Some(id.clone());
            }
            if let Some(h) = &hint {
                if name.to_lowercase().contains(h) || id.to_lowercase().contains(h) {
                    preferred = Some(id);
                    break;
                }
            }
        }
    }

    // FIX: stop() returns (), not Result
    mon.stop();
    preferred.or(first_monitor)
}

/* ------------------------------------------------------------------------- */
/* Sender                                                                     */
/* ------------------------------------------------------------------------- */

/// Build an Opus-over-RTP sender.
/// macOS: normally omit `device_name` and set System Input = BlackHole 2ch.
/// Linux: by default we pick a `.monitor` device (system audio), not the mic.
pub fn build_sender(device_name: Option<&str>, host: &str, port: u16) -> Result<Sender> {
    let pipeline = gst::Pipeline::new();

    // ---------- Source selection ----------
    #[cfg(target_os = "macos")]
    let src = {
        let s = make_element("osxaudiosrc", "src")?;
        // Good macOS defaults (your proven values)
        let src_buf_us: u64 = env::var("AB_SRC_BUFFER_US").ok().and_then(|v| v.parse().ok()).unwrap_or(200_000);
        let src_lat_us: u64 = env::var("AB_SRC_LATENCY_US").ok().and_then(|v| v.parse().ok()).unwrap_or(10_000);
        if s.has_property("buffer-time", None) {
            s.set_property("buffer-time", src_buf_us);
            eprintln!("[sender] src.buffer-time={} us", src_buf_us);
        }
        if s.has_property("latency-time", None) {
            s.set_property("latency-time", src_lat_us);
            eprintln!("[sender] src.latency-time={} us", src_lat_us);
        }
        if let Some(name) = device_name {
            if s.has_property("device", None) {
                if let Ok(idx) = name.parse::<i32>() {
                    s.set_property("device", idx);
                    eprintln!("[sender] set device index={idx}");
                } else {
                    eprintln!("[sender][warn] macOS '--capture-device' must be an integer index; got '{name}'");
                }
            }
        }
        s
    };

    #[cfg(target_os = "linux")]
    let src = {
        let s = make_element("pulsesrc", "src")?;
        if let Some(dev) = device_name {
            if s.has_property("device", None) {
                s.set_property("device", dev);
                eprintln!("[linux] pulsesrc.device='{}' (from --capture-device)", dev);
            }
        } else {
            let hint = env::var("MONITOR_HINT").ok();
            match pick_pulse_monitor(hint.as_deref()) {
                Some(dev) => {
                    if s.has_property("device", None) {
                        s.set_property("device", dev.as_str());
                        eprintln!("[linux] using monitor device='{}'", dev);
                    }
                }
                None => {
                    eprintln!("[linux][warn] no monitor source found; falling back to default pulsesrc (may be the mic)");
                }
            }
        }
        if let Ok(v) = env::var("AB_SRC_BUFFER_US").and_then(|v| v.parse::<u64>().map_err(|_| env::VarError::NotPresent)) {
            if s.has_property("buffer-time", None) {
                s.set_property("buffer-time", v);
                eprintln!("[sender] src.buffer-time={} us", v);
            }
        }
        if let Ok(v) = env::var("AB_SRC_LATENCY_US").and_then(|v| v.parse::<u64>().map_err(|_| env::VarError::NotPresent)) {
            if s.has_property("latency-time", None) {
                s.set_property("latency-time", v);
                eprintln!("[sender] src.latency-time={} us", v);
            }
        }
        s
    };

    // ---------- Format normalize & caps ----------
    let q_src = make_element("queue", "q_src")?;
    q_src.set_property("max-size-buffers", 0u32);
    q_src.set_property("max-size-bytes", 0u32);
    q_src.set_property("max-size-time", 20_000_000u64);

    let convert = make_element("audioconvert", "aconv")?;
    let resample = make_element("audioresample", "ares")?;

    let caps = gst::Caps::builder("audio/x-raw")
        .field("rate", 48_000i32)
        .field("channels", 2i32)
        .field("format", "S16LE")
        .field("layout", "interleaved")
        .build();
    let capsfilter = make_element("capsfilter", "acaps")?;
    capsfilter.set_property("caps", &caps);
    eprintln!("[sender] enforce caps: {}", caps.to_string());

    // Live meter of captured audio (before encode)
    let level_tx = make_element("level", "level_tx")?;
    if level_tx.has_property("interval", None) {
        level_tx.set_property("interval", 100_000_000u64);
    }
    if level_tx.has_property("post-messages", None) {
        level_tx.set_property("post-messages", true);
    }

    // ---------- Opus enc + RTP + UDP ----------
    let opusenc = make_element("opusenc", "opusenc")?;
    opusenc.set_property("bitrate", 256_000i32);
    opusenc.set_property("inband-fec", false);
    if opusenc.has_property("frame-size", None) {
        opusenc.set_property_from_str("frame-size", "2.5");
    }
    if opusenc.has_property("complexity", None) {
        opusenc.set_property("complexity", 5i32);
        eprintln!("[sender] opusenc.complexity=5");
    }
    eprintln!("[sender] opusenc: bitrate=256000, frame-size=2.5ms");

    let pay = make_element("rtpopuspay", "pay")?;
    pay.set_property("pt", 97u32);

    let sink = make_element("udpsink", "udpsink")?;
    sink.set_property("host", host);
    sink.set_property("port", port as i32);
    sink.set_property("sync", false);
    sink.set_property("async", false);
    eprintln!("[sender] udpsink → {host}:{port}");

    // ---------- Build & link ----------
    pipeline.add_many(&[
        &src, &q_src, &convert, &resample, &capsfilter, &level_tx, &opusenc, &pay, &sink,
    ])?;
    gst::Element::link_many(&[
        &src, &q_src, &convert, &resample, &capsfilter, &level_tx, &opusenc, &pay, &sink,
    ])?;

    attach_caps_probe(&src, "src", "snd/src");
    attach_caps_probe(&opusenc, "src", "snd/opus");
    attach_caps_probe(&pay, "src", "snd/rtp");
    attach_tx_stats(&pay, "src", "sender");

    attach_bus_logging(&pipeline, "sender");
    eprintln!("[sender] pipeline built");
    Ok(Sender { pipeline })
}

/* ------------------------------------------------------------------------- */
/* Receiver                                                                   */
/* ------------------------------------------------------------------------- */

pub fn build_receiver(listen_port: u16) -> Result<Receiver> {
    let pipeline = gst::Pipeline::new();

    let src = make_element("udpsrc", "udpsrc")?;
    src.set_property("port", listen_port as i32);

    let rtp_caps = gst::Caps::builder("application/x-rtp")
        .field("media", "audio")
        .field("encoding-name", "OPUS")
        .field("clock-rate", 48_000i32)
        .field("payload", 97i32)
        .build();
    src.set_property("caps", &rtp_caps);
    eprintln!(
        "[recv] udpsrc listening on :{} with caps {}",
        listen_port,
        rtp_caps.to_string()
    );

    let q_net = make_element("queue", "q_net")?;
    q_net.set_property("max-size-buffers", 0u32);
    q_net.set_property("max-size-bytes", 0u32);
    q_net.set_property("max-size-time", 20_000_000u64);

    let jitter = make_element("rtpjitterbuffer", "jbuf")?;
    let jitter_ms: u32 = env::var("JITTER_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    if jitter.has_property("latency", None) {
        jitter.set_property("latency", jitter_ms);
        eprintln!("[recv] jbuf.latency={} ms", jitter_ms);
    }
    if jitter.has_property("drop-on-late", None) {
        let drop_on_late = env::var("DROP_ON_LATE").map(|v| v == "1").unwrap_or(true);
        jitter.set_property("drop-on-late", drop_on_late);
        eprintln!("[recv] jbuf.do-lost=true");
    }
    if jitter.has_property("do-lost", None) {
        jitter.set_property("do-lost", true);
        eprintln!("[recv] jbuf.do-lost=true");
    }

    let depay = make_element("rtpopusdepay", "depay")?;
    let dec = make_element("opusdec", "opusdec")?;
    if dec.has_property("plc", None) {
        let plc = env::var("PLC").map(|v| v == "1").unwrap_or(false);
        dec.set_property("plc", plc);
        eprintln!("[recv] opusdec.plc={plc}");
    }
    let convert = make_element("audioconvert", "aconv")?;
    let resample = make_element("audioresample", "ares")?;

    let level = make_element("level", "level")?;
    if level.has_property("interval", None) {
        level.set_property("interval", 100_000_000u64);
    }
    if level.has_property("post-messages", None) {
        level.set_property("post-messages", true);
    }

    let q_sink = make_element("queue", "q_sink")?;
    q_sink.set_property("max-size-buffers", 0u32);
    q_sink.set_property("max-size-bytes", 0u32);
    q_sink.set_property("max-size-time", 20_000_000u64);

    let sink = if cfg!(target_os = "macos") {
        make_element("osxaudiosink", "sink")?
    } else if let Ok(dev) = env::var("PULSE_SINK") {
        eprintln!("[recv] using pulsesink device='{dev}' (PULSE_SINK)");
        let s = make_element("pulsesink", "sink")?;
        if s.has_property("device", None) {
            s.set_property("device", dev);
        }
        s
    } else if env::var("AUTO_SINK").as_deref() == Ok("1") {
        eprintln!("[recv] using autoaudiosink (AUTO_SINK=1)");
        make_element("autoaudiosink", "sink")?
    } else {
        eprintln!("[recv] using pulsesink (default)");
        make_element("pulsesink", "sink")?
    };

    let sink_buf_us: u64 = env::var("SINK_BUFFER_US").ok().and_then(|v| v.parse().ok()).unwrap_or(70_000);
    let sink_lat_us: u64 = env::var("SINK_LATENCY_US").ok().and_then(|v| v.parse().ok()).unwrap_or(15_000);
    if sink.has_property("buffer-time", None) {
        sink.set_property("buffer-time", sink_buf_us);
        eprintln!("[recv] sink.buffer-time={} us", sink_buf_us);
    }
    if sink.has_property("latency-time", None) {
        sink.set_property("latency-time", sink_lat_us);
        eprintln!("[recv] sink.latency-time={} us", sink_lat_us);
    }
    if sink.has_property("sync", None) {
        let sync = env::var("SINK_SYNC").map(|v| v != "0").unwrap_or(true);
        sink.set_property("sync", sync);
        eprintln!("[recv] sink.sync={sync}");
    }

    pipeline.add_many(&[
        &src, &q_net, &jitter, &depay, &dec, &convert, &resample, &level, &q_sink, &sink,
    ])?;
    gst::Element::link_many(&[
        &src, &q_net, &jitter, &depay, &dec, &convert, &resample, &level, &q_sink, &sink,
    ])?;

    attach_caps_probe(&depay, "src", "rcv/opus");
    attach_caps_probe(&sink, "sink", "rcv/sink");

    attach_bus_logging(&pipeline, "receiver");
    eprintln!("[recv] pipeline built");
    Ok(Receiver { pipeline })
}

/* ------------------------------------------------------------------------- */
/* Start / Stop                                                               */
/* ------------------------------------------------------------------------- */

impl Sender {
    pub fn start(&self) -> Result<()> {
        eprintln!("[sender] starting…");
        self.pipeline
            .set_state(gst::State::Playing)
            .context("sender: set_state(Playing)")?;
        eprintln!("[sender] started");
        Ok(())
    }
    pub fn stop(&self) {
        eprintln!("[sender] stopping…");
        let _ = self.pipeline.set_state(gst::State::Null);
        eprintln!("[sender] stopped");
    }
}

impl Receiver {
    pub fn start(&self) -> Result<()> {
        eprintln!("[recv] starting…");
        self.pipeline
            .set_state(gst::State::Playing)
            .context("receiver: set_state(Playing)")?;
        eprintln!("[recv] started");
        Ok(())
    }
    pub fn stop(&self) {
        eprintln!("[recv] stopping…");
        let _ = self.pipeline.set_state(gst::State::Null);
        eprintln!("[recv] stopped");
    }
}
