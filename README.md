# Audio Bridge (Rust, v1)

A cross-platform, low-latency LAN audio bridge between **Linux** and **macOS**.  
It captures **“what you hear”** (system or app audio), encodes with Opus, ships over RTP/UDP, and plays it on the other machine.

Think “DIY Dante Lite” for home/office, written in Rust.

---

## Features (v1)
- Bidirectional streaming (run on both machines).
- Captures system audio (via BlackHole on macOS, PipeWire loopback on Linux).
- Low-latency Opus encode (2.5–5 ms frames).
- Auto-playback on system speakers.
- mDNS advertisement (peers show up automatically).

---

## 1. Setup

### macOS
1. Run the setup script:
   ```bash
   cd scripts
   ./macos_setup.sh
```

This installs:

* GStreamer + plugins
* BlackHole 2ch (virtual audio device)
* SwitchAudioSource CLI

2. In **Audio MIDI Setup**:

   * Create a **Multi-Output Device** with:

     * ✅ BlackHole 2ch
     * ✅ Your speakers (e.g., MacBook Speakers, Headphones)
   * Right-click → *Use This Device For Sound Output*.
     This mirrors everything you hear to both your speakers and BlackHole.

3. To route audio later:

   ```bash
   ./macos_setup.sh route_system_to_multi
   ./macos_setup.sh route_system_to_builtin
   ```

### Linux (PipeWire)

1. Run the setup script:

   ```bash
   cd scripts
   ./linux_setup.sh
   ```

   This installs:

   * GStreamer + plugins
   * PipeWire + WirePlumber
   * Pulseaudio utils (for pactl)

   It also creates a **null sink** called `bridge_out` and sets it as default.

2. All playback now goes into `bridge_out`. Capture the monitor source:

   * Run `./linux_setup.sh list_sources`
   * Look for `bridge_out.monitor` — that’s your TX source.

3. To remove later:

   ```bash
   ./linux_setup.sh remove_bridge_sink
   ```

---

## 2. Build the Daemon

Install Rust (1.75+ recommended), then:

```bash
cargo build --release
```

Executable lives at:

```
target/release/ab-daemon
```

---

## 3. Run it

### On Linux (receiver only):

```bash
./target/release/ab-daemon --listen_port 5004
```

### On macOS (sender + receiver):

```bash
./target/release/ab-daemon \
  --capture_device "BlackHole 2ch" \
  --send_to <LINUX_IP> \
  --send_port 5002 \
  --listen_port 5004
```

* Replace `<LINUX_IP>` with the LAN IP of your Linux box.
* Run the reverse command on Linux (with `--send_to <MAC_IP>`) for full duplex.

---

## 4. Quick Test (without Rust daemon)

You can also sanity-check with raw GStreamer:

**macOS → Linux**

```bash
# macOS (sender)
gst-launch-1.0 osxaudiosrc device="BlackHole 2ch" \
  ! audioconvert ! audioresample \
  ! opusenc frame-size=2.5 bitrate=256000 \
  ! rtpopuspay pt=97 \
  ! udpsink host=<LINUX_IP> port=5004

# Linux (receiver)
gst-launch-1.0 udpsrc port=5004 caps="application/x-rtp,media=audio,encoding-name=OPUS,clock-rate=48000,pt=97" \
  ! rtpjitterbuffer latency=10 drop-on-late=true do-lost=true \
  ! rtpopusdepay ! opusdec \
  ! audioconvert ! audioresample ! pipewiresink
```

---

## Coming Next

* **Drift correction**
  Adaptive resampling to stay perfectly in sync across devices.

* **Security**
  Optional SRTP with a shared key for LAN, later DTLS-SRTP for WAN.

* **WebRTC transport**
  Use WebRTC for WAN/NAT traversal, keeping RTP/UDP as “LAN turbo mode.”

* **UI (Tauri app)**
  Tray icon with peer discovery, connect/disconnect, and level meters.

* **Advanced routing**
  Per-app capture on macOS (Loopback), multi-room playback (multicast).

---

## License

MIT

```

