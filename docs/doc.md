# Audio Bridge - V1 Summary

> Goal: a tiny, low-latency, cross-platform (macOS ↔ Linux) **system-audio bridge** over LAN. Capture “what you hear”, encode to **Opus**, wrap in **RTP**, send over **UDP**, and play on the other machine.

---

## What we built

* **Rust workspace** with a minimal daemon and a GStreamer-based core:

  * `crates/core`: the **media pipeline** (sender & receiver) + verbose logging.
  * `crates/daemon`: CLI (args), (optional) mDNS scaffolding, main loop.
* **Install scripts**:

  * `scripts/macos_setup.sh`: installs GStreamer, **BlackHole 2ch**, SwitchAudioSource.
  * `scripts/linux_setup.sh`: installs GStreamer, PipeWire/WirePlumber, `pactl`; creates a `bridge_out` **null sink** (as default) to offer a `.monitor` source.
* **README** with quick start, sanity checks, and troubleshooting.

---

## Key things we debugged & fixed

* **Rust edition/toolchain**: removed `edition2024` requirement to work with stable (you’re on Cargo 1.78).
* **GStreamer property typing**:

  * `osxaudiosrc device`: expects **gint** (index), not string.
  * `opusenc frame-size`: enum → set via **string** (e.g., `"2.5"`).
  * `rtpopuspay pt`: expects **guint** (u32).
  * `udpsink port`: **gint** (i32).
* **Receiver jitter buffer props**: feature-check `drop-on-late` / `do-lost` because older builds may miss them.
* **Silence on macOS**: fixed by matching **working CoreAudio timings**:

  * `AB_SRC_BUFFER_US=200000`
  * `AB_SRC_LATENCY_US=10000`
* **VS Code mic permission quirks**: run from **Terminal** first so TCC prompts for the binary.
* **Linux playback routing**: ensure we target a **real sink** (or set `PULSE_SINK`) so playback doesn’t land on a null/bridge sink.
* **Linux sender noise**: default to **system monitor** (not mic) by auto-selecting a Pulse/pipewire `.monitor` source.

---

## Current behavior (v1)

* **Codec/transport**: Opus @ 48 kHz stereo, 2.5 ms frames, payload type **97**, RTP over UDP.
* **macOS sender**: `osxaudiosrc`, defaults to **buffer=200 ms / latency=10 ms** (overridable).
* **Linux sender**: `pulsesrc` that **auto-picks a monitor** source (system audio). Override with `--capture-device` if needed.
* **Receiver**: `udpsrc → rtpjitterbuffer → rtpopusdepay → opusdec → (convert/resample) → sink`.
* **Logging**: deep, readable, and helpful (caps, element messages, TX stats, levels).

---

## File/Code structure

```
audio-bridge/
├─ crates/
│  ├─ core/
│  │  └─ src/pipeline.rs     # GStreamer sender/receiver builders + logging helpers
│  └─ daemon/
│     ├─ src/main.rs         # CLI wiring + start sender/receiver
│     ├─ src/args.rs         # clap-based args definition
│     └─ src/mdns.rs         # zeroconf scaffolding (compile-ready; optional)
├─ scripts/
│  ├─ macos_setup.sh         # brew installs, BlackHole, SwitchAudioSource
│  └─ linux_setup.sh         # pipewire setup, creates bridge_out null sink
└─ README.md                 # how to set up, run, sanity check, troubleshoot
```

---

## Code — detailed design notes

### 1) Core Media Pipelines (`crates/core/src/pipeline.rs`)

#### Shared utilities

* `make_element(factory, name) -> gst::Element`

  * Creates an element, names it, logs `"[build] created {factory} as {name}"`.
* `attach_bus_logging(pipeline, tag)`

  * Dedicated thread reading the pipeline bus. Prints:

    * **ERROR/WARN/INFO** with element paths and debug strings.
    * **StateChanged** transitions (useful for PREROLL & PLAYING).
    * **Element** messages (e.g., from `level`).
    * **Latency** notifications.
* `attach_caps_probe(elem, pad_name, tag)`

  * Adds a pad probe to log **caps** changes: `"[caps:{tag}] {pad} -> {caps}"`.
* `attach_tx_stats(elem, pad_name, tag)`

  * Counts packets/bytes on a pad and logs **\~1s** throughput (pkts/s, kbit/s).

#### Sender (Opus-over-RTP)

**macOS path**

```
osxaudiosrc [device=<index>, buffer-time, latency-time]
  → queue (q_src)
  → audioconvert
  → audioresample
  → capsfilter (audio/x-raw, rate=48000, channels=2, format=S16LE, layout=interleaved)
  → level (level_tx)   # meters the captured audio (should not be -700/-350)
  → opusenc [bitrate=256k, frame-size="2.5", complexity=5, inband-fec=false]
  → rtpopuspay [pt=97]
  → udpsink [host, port, sync=false, async=false]
```

* **Device selection**: `--capture-device <index>` for `osxaudiosrc` (gint).
* **Timing defaults** (critical on macOS):

  * `AB_SRC_BUFFER_US` (default **200000**)
  * `AB_SRC_LATENCY_US` (default **10000**)
* **Important log lines**:

  * `[sender] set device index=…`
  * `[caps:snd/src] …` / `[caps:snd/opus] …` / `[caps:snd/rtp] …`
  * `[sender] TX ~… pkts/s, ~… kbit/s …`
  * `ELEMENT level` from `level_tx` shows **rms/peak** (should move; silence is `-700/-350`)

**Linux path (system audio by default)**

```
pulsesrc [device="<monitor-id>" auto-picked]
  → queue (q_src)
  → audioconvert
  → audioresample
  → capsfilter (audio/x-raw, rate=48000, channels=2, format=S16LE, layout=interleaved)
  → level (level_tx)
  → opusenc (same as macOS)
  → rtpopuspay [pt=97]
  → udpsink [host, port, sync=false, async=false]
```

* **Monitor auto-pick**:

  * Uses `gst::DeviceMonitor` to list `Audio/Source` devices and choose one whose display name or properties **contain `"monitor"`** (e.g., `alsa_output.pci-…analog-stereo.monitor` or `bridge_out.monitor`).
  * `MONITOR_HINT=<substring>` (optional) biases the selection (e.g., `analog`, `hdmi`, `bridge_out`).
  * `--capture-device <pulse_device_name>` overrides the auto-pick.
* **Optional timing**:

  * `AB_SRC_BUFFER_US` / `AB_SRC_LATENCY_US` can be set if you want to tune Linux capture.

#### Receiver (RTP → PCM → speakers)

```
udpsrc [port, caps="application/x-rtp, ... , payload=97"]
  → queue (q_net)
  → rtpjitterbuffer [latency=JITTER_MS, drop-on-late=?, do-lost=?]
  → rtpopusdepay
  → opusdec [plc=?]
  → audioconvert
  → audioresample
  → level
  → queue (q_sink)
  → sink (macOS: osxaudiosink; Linux: pulsesink | autoaudiosink)
```

* **RTP caps**: enforced on `udpsrc` (`payload=97`, `encoding-name=OPUS`, `clock-rate=48000`).
* **Jitter & sink tuning (env)**:

  * `JITTER_MS` (default **30**)
  * `SINK_BUFFER_US` (default **70000**)
  * `SINK_LATENCY_US` (default **15000**)
  * `SINK_SYNC` (default **1** true; set `0` for async)
* **Sink selection (Linux)**:

  * `PULSE_SINK=<name>` pins a particular sink (e.g., your real speakers).
  * `AUTO_SINK=1` uses `autoaudiosink`; default is `pulsesink`.

---

### 2) Daemon (`crates/daemon`)

* **Args** (`args.rs`) — examples you’ve used:

  * `--listen-port <port>`: start receiver.
  * `--send-to <ip> --send-port <port>`: start sender.
  * `--capture-device <value>`:

    * macOS: **integer** device index for `osxaudiosrc`.
    * Linux: **Pulse device string** (e.g., `bridge_out.monitor`).
* **Main** (`main.rs`):

  * Parses args, builds pipelines via `ab-core`, and drives **start()/stop()**.
  * Includes optional tokio `ctrl_c` handling (enabled by compiling tokio with `signal` feature).
* **mDNS scaffolding** (`mdns.rs`):

  * Uses `zeroconf` (`ServiceType::new("_audiobridge", "_udp")`, `TMdnsService`, `TTxtRecord` traits).
  * Implements **register** and **browse** helpers (compile-clean after trait import fixes).
  * Not strictly required for v1 — can be wired in later to auto-discover peers.

---

## Environment variables (today)

**Sender (macOS & Linux)**

* `AB_SRC_BUFFER_US` (mac default **200000**)
* `AB_SRC_LATENCY_US` (mac default **10000**)

**Sender (Linux only)**

* `MONITOR_HINT=<substring>` to bias monitor choice.

**Receiver**

* `JITTER_MS` (default **30**)
* `SINK_BUFFER_US` (default **70000**)
* `SINK_LATENCY_US` (default **15000**)
* `SINK_SYNC` (`1`=sync, `0`=async)
* `PULSE_SINK=<name>` (Linux only)
* `AUTO_SINK=1` (Linux: use `autoaudiosink`)

---

## How to run (common, working recipes)

### macOS → Linux

**macOS (sender):**

```bash
# System Output: Multi-Output (Speakers + BlackHole 2ch)
# System Input: BlackHole 2ch (not strictly required, but fine)

AB_SRC_BUFFER_US=200000 AB_SRC_LATENCY_US=10000 \
./target/release/ab-daemon \
  --capture-device <OSX_DEVICE_INDEX_FOR_BLACKHOLE> \
  --send-to 192.168.1.43 \
  --send-port 6006
```

**Linux (receiver):**

```bash
# Pin a real sink if needed:
# PULSE_SINK=alsa_output.pci-0000_00_1f.3.analog-stereo \
JITTER_MS=40 SINK_BUFFER_US=100000 SINK_LATENCY_US=20000 SINK_SYNC=1 \
./target/release/ab-daemon --listen-port 6006
```

### Linux → macOS

**Linux (sender, system audio):**

```bash
# Auto-picks a monitor source (e.g., bridge_out.monitor)
./target/release/ab-daemon --send-to <MAC_IP> --send-port 6007
# or override
# ./target/release/ab-daemon --capture-device bridge_out.monitor --send-to <MAC_IP> --send-port 6007
```

**macOS (receiver):**

```bash
./target/release/ab-daemon --listen-port 6007
```

---

## Sanity checks (pure GStreamer)

**macOS sender (works on your machine):**

```bash
gst-launch-1.0 -v \
  osxaudiosrc device=<INDEX> \
  ! audio/x-raw,rate=48000,channels=2,format=S16LE \
  ! audioconvert ! audioresample \
  ! opusenc bitrate=256000 \
  ! rtpopuspay pt=97 \
  ! udpsink host=<LINUX_IP> port=6006
```

**Linux receiver:**

```bash
gst-launch-1.0 -v \
  udpsrc port=6006 caps="application/x-rtp,media=audio,encoding-name=OPUS,clock-rate=48000,pt=97" \
  ! rtpjitterbuffer latency=30 drop-on-late=true do-lost=true \
  ! rtpopusdepay ! opusdec ! audioconvert ! audioresample ! pulsesink
```

---

## Troubleshooting crib notes

* **macOS sender is silent (level shows -700/-350)**
  Use the working timings:

  ```bash
  AB_SRC_BUFFER_US=200000 AB_SRC_LATENCY_US=10000 …
  ```

  Run from **Terminal** (not VS Code) to ensure TCC mic permission prompts.
  Verify `osxaudiosrc device=<INDEX>` matches the **same index** you used with `gst-launch`.

* **“CoreAudio device not found”**
  Check device list:

  ```bash
  gst-device-monitor-1.0 | sed -n '/Audio\/Source/,+8p'
  ```

  Pick the right **index** for BlackHole and pass it to `--capture-device`.

* **Linux receiver “plays” but no audio heard**
  Audio might be routed to a dummy/null sink. Pin it:

  ```bash
  PULSE_SINK=alsa_output.pci-0000_00_1f.3.analog-stereo ./ab-daemon --listen-port 6006
  ```

* **Linux sender is noisy (mic)**
  Let the daemon auto-pick a `.monitor` source, or force it:

  ```bash
  ./ab-daemon --capture-device bridge_out.monitor --send-to <IP> --send-port <PORT>
  ```

* **Underflows / jitter**
  Increase receiver jitter/latency:

  ```bash
  JITTER_MS=40 SINK_BUFFER_US=100000 SINK_LATENCY_US=20000
  ```

---

## README snapshot (what your repo currently says)

* Clean setup steps for macOS & Linux (no “remove sink” options).
* Build & run examples (including the macOS timing envs).
* Quick GStreamer sanity tests.
* Troubleshooting section covering silence, device selection, Linux monitor/mic.
* TODO list.

---

## TODO / Next up

* **Drift correction** (adaptive resampling / clock sync).
* **Security**: SRTP (LAN) → DTLS-SRTP (WAN).
* **WebRTC transport** (for NAT traversal).
* **UX**: Tauri tray app w/ meters, peer discovery, connect UI.
* **macOS backend toggle** (`avfaudiosrc` fallback) & select by **name** not index.
* **Service units**: systemd (Linux), launchd (macOS).
* **Per-app routing** (Loopback on macOS, PipeWire filters on Linux).
* **Installers** (brew/apt).



<!-- ## What to tell the next chat to continue smoothly

* You already have:

  * A working sender/receiver in Rust using GStreamer, with proven macOS timings.
  * Linux sender that auto-captures **monitor** (system audio), not mic.
  * Deep logging and meters; TX stats; env-tunable jitter/sink timings.
  * Scripts that set up BlackHole (macOS) and a null sink monitor (Linux).
  * A README with quick start + troubleshooting.

* You want next:

  * Drift correction & clock sync.
  * Optional SRTP.
  * macOS `avfaudiosrc` fallback + pick device by name.
  * Simple tray UI and service units. -->
