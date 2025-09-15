# Audio Bridge (Rust, v1)

A cross-platform, low-latency LAN audio bridge between **Linux** and **macOS**.  
It captures **“what you hear”** (system or app audio), encodes with **Opus**, ships over **RTP/UDP**, and plays it on the other machine.

Think “DIY Dante Lite” for home/office, written in Rust.

---

## ✨ Features
- 🔄 **Bidirectional streaming** (run on both machines).
- 🎧 Captures system audio (BlackHole on macOS, PipeWire monitor on Linux).
- ⚡ Low-latency Opus (2.5–5 ms frames).
- 🔊 Automatic playback on system speakers.
- 🌐 Optional mDNS advertisement for peer auto-discovery.

---

## 🔧 Setup

### macOS
```bash
cd scripts
./macos_setup.sh
````

This installs:

* GStreamer + plugins
* BlackHole 2ch (virtual device)
* SwitchAudioSource CLI

Then in **Audio MIDI Setup**:

* Create a **Multi-Output Device** with:

  * ✅ BlackHole 2ch
  * ✅ Your speakers (MacBook Speakers / Headphones)
* Set it as the default output.

---

### Linux (PipeWire)

```bash
cd scripts
./linux_setup.sh
```

This installs:

* GStreamer + plugins
* PipeWire + WirePlumber
* Pulseaudio utils (`pactl`)

It also creates a **null sink** called `bridge_out`.
Use its `.monitor` source as your TX input.

---

## 🛠️ Build the Daemon

Install Rust (1.75+ recommended):

```bash
cargo build --release
```

Binary is at:

```
target/release/ab-daemon
```

---

## 🚀 Run It

### Linux (receiver only):

```bash
./target/release/ab-daemon --listen-port 5004
```

### macOS (sender + receiver):

```bash
AB_SRC_BUFFER_US=200000 AB_SRC_LATENCY_US=10000 \
./target/release/ab-daemon \
  --capture-device "BlackHole 2ch" \
  --send-to <LINUX_IP> \
  --send-port 5002 \
  --listen-port 5004
```

* Replace `<LINUX_IP>` with your Linux machine’s LAN IP.
* Run the reverse command on Linux (with `--send-to <MAC_IP>`) for full duplex.

💡 **Note**: `AB_SRC_BUFFER_US` and `AB_SRC_LATENCY_US` are critical on macOS.
Start with `200000` / `10000` and tune as needed.

---

## 🔍 Quick Test (without daemon)

### macOS → Linux

```bash
# macOS (sender)
gst-launch-1.0 osxaudiosrc device="BlackHole 2ch" \
  ! audioconvert ! audioresample \
  ! opusenc frame-size=2.5 bitrate=256000 \
  ! rtpopuspay pt=97 \
  ! udpsink host=<LINUX_IP> port=5004

# Linux (receiver)
gst-launch-1.0 udpsrc port=5004 caps="application/x-rtp,media=audio,encoding-name=OPUS,clock-rate=48000,pt=97" \
  ! rtpjitterbuffer latency=30 drop-on-late=true do-lost=true \
  ! rtpopusdepay ! opusdec \
  ! audioconvert ! audioresample ! pipewiresink
```

---

## 🐞 Troubleshooting

* **Silence on macOS sender**
  Ensure you run from the **Terminal** (not VSCode), so the app has microphone permissions.
  Set `AB_SRC_BUFFER_US=200000` and `AB_SRC_LATENCY_US=10000`.

* **CoreAudio device not found**
  Check your device list with:

  ```bash
  gst-device-monitor-1.0 Audio
  ```

  Then use the correct `--capture-device <INDEX>`.

* **Linux internal mic sounds noisy**
  Use the `.monitor` source of a null sink (e.g., `bridge_out.monitor`) instead of the raw mic.

* **No audio on Linux playback**
  Confirm the RTP caps match exactly (`payload=97`, `clock-rate=48000`, `encoding-name=OPUS`).

---

## 🛣️ TODO

* 🔁 Drift correction (adaptive resampling).
* 🔒 SRTP/DTLS encryption.
* 🌍 WebRTC transport for WAN.
* 🖥️ GUI (Tauri tray app with meters).
* 🎚️ Per-app routing (Loopback on macOS, PipeWire filters on Linux).
* 📦 Better install scripts (brew/apt).

---

## 📜 License

MIT

```

---