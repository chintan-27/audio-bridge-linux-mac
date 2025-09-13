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

    // Caps probes (handy when debugging)
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

    // UDP in
    let src = make_element("udpsrc", "udpsrc")?;
    src.set_property("port", listen_port as i32); // gint
    eprintln!("[recv] udpsrc listening on :{}", listen_port);

    // Caps must match sender
    let caps = gst::Caps::builder("application/x-rtp")
        .field("media", "audio")
        .field("encoding-name", "OPUS")
        .field("clock-rate", 48_000i32)
        .field("pt", 97i32)
        .build();
    let capsfilter = make_element("capsfilter", "rcaps")?;
    capsfilter.set_property("caps", &caps);
    eprintln!("[recv] expect RTP caps: {}", caps.to_string());

    // Jitter buffer (guard properties for older builds)
    let jitter = make_element("rtpjitterbuffer", "jbuf")?;
    if jitter.has_property("latency", None) {
        jitter.set_property("latency", 10u32);
        eprintln!("[recv] jbuf.latency=10 ms");
    }
    if jitter.has_property("drop-on-late", None) {
        jitter.set_property("drop-on-late", true);
        eprintln!("[recv] jbuf.drop-on-late=true");
    }
    if jitter.has_property("do-lost", None) {
        jitter.set_property("do-lost", true);
        eprintln!("[recv] jbuf.do-lost=true");
    }

    // Depay + decode + format
    let depay = make_element("rtpopusdepay", "depay")?;
    let dec = make_element("opusdec", "opusdec")?;
    if dec.has_property("plc", None) {
        dec.set_property("plc", false);
    }
    let convert = make_element("audioconvert", "aconv")?;
    let resample = make_element("audioresample", "ares")?;

    // Sink (Linux: prefer autoaudiosink to avoid null-sink surprises)
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

    pipeline.add_many(&[
        &src, &capsfilter, &jitter, &depay, &dec, &convert, &resample, &sink,
    ])?;
    gst::Element::link_many(&[
        &src, &capsfilter, &jitter, &depay, &dec, &convert, &resample, &sink,
    ])?;

    // Caps probes
    attach_caps_probe(&src, "src", "rcv/rtp");
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
