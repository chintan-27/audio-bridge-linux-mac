use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;

pub struct Sender {
    pipeline: gst::Pipeline,
}
pub struct Receiver {
    pipeline: gst::Pipeline,
}

pub fn init_gst() -> Result<()> {
    gst::init().context("gst init failed")
}

/// Build an Opus-over-RTP sender.
/// macOS: set `device_name` = "BlackHole 2ch"
/// Linux: leave None to use default PipeWire monitor selection (see devices.rs)
pub fn build_sender(device_name: Option<&str>, host: &str, port: u16) -> Result<Sender> {
    // Elements
    let pipeline = gst::Pipeline::new();
    let src = if cfg!(target_os = "macos") {
        gst::ElementFactory::make("osxaudiosrc").build()?
    } else {
        // PipeWire source
        gst::ElementFactory::make("pipewiresrc").build()?
    };
    if let Some(name) = device_name {
        let mut set_ok = false;
        if src.has_property("device-name", None) {
            src.set_property("device-name", name);
            set_ok = true;
        } else if src.has_property("device", None) {
            if let Ok(idx) = name.parse::<i32>() {
                src.set_property("device", idx);
                set_ok = true;
            }
        }
        if !set_ok {
            eprintln!(
                "[warn] capture device hint '{}' ignored (need 'device-name' or integer 'device')",
                name
            );
        }
    }

    // Tight device buffers help latency; tune conservatively first
    if src.has_property("buffer-time", None) {
        src.set_property_from_str("buffer-time", "5000");   // microseconds
    }
    if src.has_property("latency-time", None) {
        src.set_property_from_str("latency-time", "5000");
    }

    let convert = gst::ElementFactory::make("audioconvert").build()?;
    let resample = gst::ElementFactory::make("audioresample").build()?;
    let caps = gst::Caps::builder("audio/x-raw")
        .field("rate", 48000i32)
        .field("channels", 2i32)
        .build();
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()?;

    let opusenc = gst::ElementFactory::make("opusenc")
        .property("bitrate", 256_000i32)
        .property("inband-fec", false)
        .build()?;
    // 'frame-size' is an enum (GstOpusEncFrameSize). Set it via string to avoid type mismatch.
    if opusenc.has_property("frame-size", None) {
        // accepted values include: "2.5", "5", "10", "20", "40", "60"
        opusenc.set_property_from_str("frame-size", "2.5");
    }
    let pay = gst::ElementFactory::make("rtpopuspay")
        .property("pt", 97u32)   // guint
        .build()?;
    let sink = gst::ElementFactory::make("udpsink")
        .property("host", host)
        .property("port", port as i32)  // guint
        .property("sync", false)
        .property("async", false)
        .build()?;

    pipeline.add_many(&[&src, &convert, &resample, &capsfilter, &opusenc, &pay, &sink])?;
    gst::Element::link_many(&[&src, &convert, &resample, &capsfilter, &opusenc, &pay, &sink])?;

    Ok(Sender { pipeline })
}

pub fn build_receiver(listen_port: u16) -> Result<Receiver> {
    let pipeline = gst::Pipeline::new();

    let src = gst::ElementFactory::make("udpsrc")
        .property("port", listen_port as i32)
        .build()?;

    // Caps must match sender
    let caps = gst::Caps::builder("application/x-rtp")
        .field("media", "audio")
        .field("encoding-name", "OPUS")
        .field("clock-rate", 48_000i32)
        .field("pt", 97i32)
        .build();
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()?;

    let jitter = gst::ElementFactory::make("rtpjitterbuffer").build()?;
    // Some distros ship older rtpjitterbuffer without certain props.
    if jitter.has_property("latency", None) {
        jitter.set_property("latency", 10u32);
    }
    if jitter.has_property("drop-on-late", None) {
        jitter.set_property("drop-on-late", true);
    }
    if jitter.has_property("do-lost", None) {
        jitter.set_property("do-lost", true);
    }


    let depay = gst::ElementFactory::make("rtpopusdepay").build()?;
    let dec = gst::ElementFactory::make("opusdec")
        .property("plc", false)
        .build()?;
    let convert = gst::ElementFactory::make("audioconvert").build()?;
    let resample = gst::ElementFactory::make("audioresample").build()?;

    let sink = if cfg!(target_os = "macos") {
        gst::ElementFactory::make("osxaudiosink")
            // Reduce added buffering if your build supports it
            .build()?
    } else {
        // NOTE: If your default sink is a null sink (e.g., "bridge_out"),
        // this will be silent. You can:
        //  - change the default sink with `pactl set-default-sink <real_sink>`, or
        //  - set a specific device here via property (if supported by your build), or
        //  - temporarily replace with `autoaudiosink` for testing.
        gst::ElementFactory::make("pipewiresink").build()?
        // For testing: `gst::ElementFactory::make("autoaudiosink").build()?`
    };

    pipeline.add_many(&[&src, &capsfilter, &jitter, &depay, &dec, &convert, &resample, &sink])?;
    gst::Element::link_many(&[&src, &capsfilter, &jitter, &depay, &dec, &convert, &resample, &sink])?;

    Ok(Receiver { pipeline })
}

impl Sender {
    pub fn start(&self) -> Result<()> {
        self.pipeline.set_state(gst::State::Playing)?;
        Ok(())
    }
    pub fn stop(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
impl Receiver {
    pub fn start(&self) -> Result<()> {
        self.pipeline.set_state(gst::State::Playing)?;
        Ok(())
    }
    pub fn stop(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
