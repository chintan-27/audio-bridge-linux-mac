use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::env;

pub struct Sender {
    pipeline: gst::Pipeline,
}
pub struct Receiver {
    pipeline: gst::Pipeline,
}

/* ------------------------- utilities & logging -------------------------- */

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

pub fn init_gst() -> Result<()> {
    gst::init().context("gst init failed")?;
    eprintln!(
        "[init] GStreamer {} (GST_PLUGIN_PATH={:?})",
        gst::version_string(),
        env::var("GST_PLUGIN_PATH").ok()
    );
    Ok(())
}

/* ------------------------------ sender ---------------------------------- */

/// Build an Opus-over-RTP sender.
/// macOS: normally omit `device_name` and set **System Input = BlackHole 2ch**.
/// If you pass a device, `osxaudiosrc.device` is an **integer index**.
pub fn build_sender(device_name: Option<&str>, host: &str, port: u16) -> Result<Sender> {
    let pipeline = gst::Pipeline::new();

    // Source
    let src = if cfg!(target_os = "macos") {
        make_element("osxaudiosrc", "src")?
    } else {
        make_element("pipewiresrc", "src")?
    };

    // Device selection (safe)
    if let Some(name) = device_name {
        let mut set_ok = false;
        if src.has_property("device-name", None) {
            src.set_property("device-name", name);
            eprintln!("[sender] set device-name='{}'", name);
            set_ok = true;
        } else if src.has_property("device", None) {
            if let Ok(idx) = name.parse::<i32>() {
                src.set_property("device", idx);
                eprintln!("[sender] set device index={}", idx);
                set_ok = true;
            } else {
                eprintln!("[sender] note: 'device' expects integer index; got '{}'", name);
            }
        }
        if !set_ok {
            eprintln!(
                "[sender][warn] capture device hint '{}' ignored (need 'device-name' or integer 'device')",
                name
            );
        }
    }

    // Tight device buffers (if supported)
    if src.has_property("buffer-time", None) {
        src.set_property_from_str("buffer-time", "5000"); // µs
        eprintln!("[sender] src.buffer-time=5000");
    }
    if src.has_property("latency-time", None) {
        src.set_property_from_str("latency-time", "5000");
        eprintln!("[sender] src.latency-time=5000");
    }

    // Format normalize before Opus (low-latency canonical format)
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

    // Opus encode
    let opusenc = make_element("opusenc", "opusenc")?;
    opusenc.set_property("bitrate", 256_000i32);
    opusenc.set_property("inband-fec", false);
    if opusenc.has_property("frame-size", None) {
        opusenc.set_property_from_str("frame-size", "2.5"); // parsed enum
    }
    eprintln!("[sender] opusenc: bitrate=256000, frame-size=2.5ms");

    // RTP pay + UDP out
    let pay = make_element("rtpopuspay", "pay")?;
    pay.set_property("pt", 97u32); // guint
    let sink = make_element("udpsink", "udpsink")?;
    sink.set_property("host", host);
    sink.set_property("port", port as i32); // gint
    sink.set_property("sync", false);
    sink.set_property("async", false);
    eprintln!("[sender] udpsink → {}:{}", host, port);

    pipeline.add_many(&[&src, &convert, &resample, &capsfilter, &opusenc, &pay, &sink])?;
    gst::Element::link_many(&[&src, &convert, &resample, &capsfilter, &opusenc, &pay, &sink])?;

    // Caps probes
    attach_caps_probe(&src, "src", "snd/src");
    attach_caps_probe(&opusenc, "src", "snd/opus");
    attach_caps_probe(&pay, "src", "snd/rtp");

    attach_bus_logging(&pipeline, "sender");
    eprintln!("[sender] pipeline built");
    Ok(Sender { pipeline })
}

/* ----------------------------- receiver --------------------------------- */

pub fn build_receiver(listen_port: u16) -> Result<Receiver> {
    let pipeline = gst::Pipeline::new();

    // UDP in + fixed RTP caps on udpsrc (use 'payload' for compatibility)
    let src = make_element("udpsrc", "udpsrc")?;
    src.set_property("port", listen_port as i32); // gint
    let rtp_caps = gst::Caps::builder("application/x-rtp")
        .field("media", "audio")
        .field("encoding-name", "OPUS")
        .field("clock-rate", 48_000i32)
        .field("payload", 97i32)
        .build();
    src.set_property("caps", &rtp_caps);
    eprintln!("[recv] udpsrc listening on :{} with caps {}", listen_port, rtp_caps.to_string());

    // Small decoupling queue right after network
    let q_net = make_element("queue", "q_net")?;
    // time-based queue (no hard caps on buffers/bytes)
    q_net.set_property("max-size-buffers", 0u32);
    q_net.set_property("max-size-bytes", 0u32);
    q_net.set_property("max-size-time", 20_000_000u64); // 20 ms

    // Jitter buffer (tunable)
    let jitter = make_element("rtpjitterbuffer", "jbuf")?;
    let jitter_ms: u32 = env::var("JITTER_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20); // default 20 ms (was 10)
    if jitter.has_property("latency", None) {
        jitter.set_property("latency", jitter_ms);
        eprintln!("[recv] jbuf.latency={} ms", jitter_ms);
    }
    if jitter.has_property("drop-on-late", None) {
        // Optional: disable if you prefer late frames over drops
        let drop_on_late = env::var("DROP_ON_LATE").map(|v| v == "1").unwrap_or(true);
        jitter.set_property("drop-on-late", drop_on_late);
        eprintln!("[recv] jbuf.drop-on-late={}", drop_on_late);
    }
    if jitter.has_property("do-lost", None) {
        jitter.set_property("do-lost", true);
        eprintln!("[recv] jbuf.do-lost=true");
    }

    // Depay + decode + format
    let depay = make_element("rtpopusdepay", "depay")?;
    let dec = make_element("opusdec", "opusdec")?;
    if dec.has_property("plc", None) {
        // You can enable PLC to mask losses: set PLC=1
        let plc = env::var("PLC").map(|v| v == "1").unwrap_or(false);
        dec.set_property("plc", plc);
        eprintln!("[recv] opusdec.plc={}", plc);
    }
    let convert = make_element("audioconvert", "aconv")?;
    let resample = make_element("audioresample", "ares")?;

    // Another tiny queue before sink to absorb sink scheduling
    let q_sink = make_element("queue", "q_sink")?;
    q_sink.set_property("max-size-buffers", 0u32);
    q_sink.set_property("max-size-bytes", 0u32);
    q_sink.set_property("max-size-time", 20_000_000u64); // 20 ms

    // Sink selection (AUTO_SINK=1 to force autoaudiosink)
    let sink = if cfg!(target_os = "macos") {
        make_element("osxaudiosink", "sink")?
    } else {
        match env::var("AUTO_SINK").as_deref() {
            Ok("1") => {
                eprintln!("[recv] using autoaudiosink (AUTO_SINK=1)");
                make_element("autoaudiosink", "sink")?
            }
            _ => {
                eprintln!("[recv] using pipewiresink (set AUTO_SINK=1 to override)");
                make_element("pipewiresink", "sink")?
            }
        }
    };

    // Modest sink buffers (us); override via env if you like
    let sink_buf_us: i64 = env::var("SINK_BUFFER_US")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40_000); // 40 ms
    let sink_lat_us: i64 = env::var("SINK_LATENCY_US")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000); // 10 ms
    if sink.has_property("buffer-time", None) {
        sink.set_property("buffer-time", sink_buf_us);
        eprintln!("[recv] sink.buffer-time={} us", sink_buf_us);
    }
    if sink.has_property("latency-time", None) {
        sink.set_property("latency-time", sink_lat_us);
        eprintln!("[recv] sink.latency-time={} us", sink_lat_us);
    }

    // Optionally bypass strict clock sync at the sink (SINK_SYNC=0)
    if sink.has_property("sync", None) {
        let sync = env::var("SINK_SYNC").map(|v| v != "0").unwrap_or(true);
        sink.set_property("sync", sync);
        eprintln!("[recv] sink.sync={}", sync);
    }

    pipeline.add_many(&[
        &src, &q_net, &jitter, &depay, &dec, &convert, &resample, &q_sink, &sink,
    ])?;
    gst::Element::link_many(&[
        &src, &q_net, &jitter, &depay, &dec, &convert, &resample, &q_sink, &sink,
    ])?;

    // Probes
    attach_caps_probe(&depay, "src", "rcv/opus");
    attach_caps_probe(&sink, "sink", "rcv/sink");

    attach_bus_logging(&pipeline, "receiver");
    eprintln!("[recv] pipeline built");
    Ok(Receiver { pipeline })
}

/* --------------------------- start / stop -------------------------------- */

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
