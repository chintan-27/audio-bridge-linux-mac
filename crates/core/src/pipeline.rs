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
                        if s.name() == "level" {
                            let rms = s.get_value("rms").ok();
                            let peak = s.get_value("peak").ok();
                            eprintln!(
                                "[{tag}] LEVEL rms={:?}, peak={:?}",
                                rms.unwrap_or_default(),
                                peak.unwrap_or_default()
                            );
                        }
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
/* Sender                                                                     */
/* ------------------------------------------------------------------------- */

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

    // Relax source buffers (avoid "Can't record audio fast enough" on macOS)
    if src.has_property("buffer-time", None) {
        src.set_property("buffer-time", 20_000i64); // µs
        eprintln!("[sender] src.buffer-time=20000");
    }
    if src.has_property("latency-time", None) {
        src.set_property("latency-time", 20_000i64); // µs
        eprintln!("[sender] src.latency-time=20000");
    }

    // Decouple source from encoder with a tiny time-based queue
    let q_src = make_element("queue", "q_src")?;
    q_src.set_property("max-size-buffers", 0u32);
    q_src.set_property("max-size-bytes", 0u32);
    q_src.set_property("max-size-time", 20_000_000u64); // 20 ms

    // Normalize format before Opus (low-latency canonical format)
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
    if opusenc.has_property("complexity", None) {
        opusenc.set_property("complexity", 5i32); // lighter CPU than default 9
        eprintln!("[sender] opusenc.complexity=5");
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

    pipeline.add_many(&[&src, &q_src, &convert, &resample, &capsfilter, &opusenc, &pay, &sink])?;
    gst::Element::link_many(&[&src, &q_src, &convert, &resample, &capsfilter, &opusenc, &pay, &sink])?;

    // Caps probes (handy when debugging)
    attach_caps_probe(&src, "src", "snd/src");
    attach_caps_probe(&opusenc, "src", "snd/opus");
    attach_caps_probe(&pay, "src", "snd/rtp");

    attach_bus_logging(&pipeline, "sender");
    eprintln!("[sender] pipeline built");
    Ok(Sender { pipeline })
}

/* ------------------------------------------------------------------------- */
/* Receiver                                                                   */
/* ------------------------------------------------------------------------- */

pub fn build_receiver(listen_port: u16) -> Result<Receiver> {
    let pipeline = gst::Pipeline::new();

    // UDP in + fixed RTP caps on udpsrc (use 'payload' for compatibility)
    let src = make_element("udpsrc", "udpsrc")?;
    src.set_property("port", listen_port as i32); // gint
    let rtp_caps = gst::Caps::builder("application/x-rtp")
        .field("media", "audio")
        .field("encoding-name", "OPUS")
        .field("clock-rate", 48_000i32)
        .field("payload", 97i32) // <-- key (not 'pt')
        .build();
    src.set_property("caps", &rtp_caps);
    eprintln!(
        "[recv] udpsrc listening on :{} with caps {}",
        listen_port,
        rtp_caps.to_string()
    );

    // Small decoupling queue right after network
    let q_net = make_element("queue", "q_net")?;
    q_net.set_property("max-size-buffers", 0u32);
    q_net.set_property("max-size-bytes", 0u32);
    q_net.set_property("max-size-time", 20_000_000u64); // 20 ms

    // Jitter buffer (tunable via env)
    let jitter = make_element("rtpjitterbuffer", "jbuf")?;
    let jitter_ms: u32 = env::var("JITTER_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30); // default 30 ms
    if jitter.has_property("latency", None) {
        jitter.set_property("latency", jitter_ms);
        eprintln!("[recv] jbuf.latency={} ms", jitter_ms);
    }
    if jitter.has_property("drop-on-late", None) {
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

    // Sink selection:
    // - If PULSE_SINK is set: use pulsesink and route to that exact device.
    // - Else if AUTO_SINK=1: use autoaudiosink (Pulse by default).
    // - Else (fallback): use pulsesink (avoid pipewiresink quirks).
    let sink = if cfg!(target_os = "macos") {
        make_element("osxaudiosink", "sink")?
    } else if let Ok(dev) = env::var("PULSE_SINK") {
        eprintln!("[recv] using pulsesink device='{}' (PULSE_SINK)", dev);
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

    let level = make_element("level", "level")?;
    if level.has_property("interval", None) {
        level.set_property("interval", 100_000_000u64); // 100ms
    }
    if level.has_property("post-messages", None) {
        level.set_property("post-messages", true);
    }

    // Modest sink buffers (override via env if needed)
    let sink_buf_us: i64 = env::var("SINK_BUFFER_US").ok().and_then(|v| v.parse().ok()).unwrap_or(70_000);
    let sink_lat_us: i64 = env::var("SINK_LATENCY_US").ok().and_then(|v| v.parse().ok()).unwrap_or(15_000);
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
        eprintln!("[recv] sink.sync={}", sync);
    }

    pipeline.add_many(&[
        &src, &q_net, &jitter, &depay, &dec, &convert, &resample, &level, &q_sink, &sink,
    ])?;
    gst::Element::link_many(&[
        &src, &q_net, &jitter, &depay, &dec, &convert, &resample, &level, &q_sink, &sink,
    ])?;

    // Probes
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
