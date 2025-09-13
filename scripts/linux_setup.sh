#!/usr/bin/env bash
# linux_setup.sh — v1 bootstrap for the Rust LAN audio bridge (Linux / PipeWire)
# - Installs GStreamer + plugins and PipeWire client libs (distro-aware)
# - Creates a null sink "bridge_out" so you can route "what you hear" into it
# - Shows how to make its monitor the TX source; provides helpers to switch
#
# Usage:
#   chmod +x linux_setup.sh
#   ./linux_setup.sh
#
# Afterwards:
#   ./linux_setup.sh create_bridge_sink
#   ./linux_setup.sh list_sinks
#   ./linux_setup.sh set_default_sink bridge_out
#   ./linux_setup.sh move_all_playback_to bridge_out
#   ./linux_setup.sh test_rx
#   ./linux_setup.sh remove_bridge_sink

set -euo pipefail

print_header() {
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "  Linux Audio Bridge Setup (v1, PipeWire)"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
}

need_cmd() { command -v "$1" >/dev/null 2>&1; }

detect_pm() {
  if need_cmd apt; then echo "apt"; return; fi
  if need_cmd dnf; then echo "dnf"; return; fi
  if need_cmd pacman; then echo "pacman"; return; fi
  echo "unknown"
}

install_pkgs() {
  local pm
  pm="$(detect_pm)"
  echo "→ Detected package manager: $pm"

  case "$pm" in
    apt)
      sudo apt update
      sudo apt install -y \
        gstreamer1.0-tools \
        gstreamer1.0-plugins-base \
        gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad \
        libgstreamer1.0-dev \
        libgstreamer-plugins-base1.0-dev \
        pipewire-audio-client-libraries \
        pipewire \
        wireplumber \
        pulseaudio-utils
      ;;
    dnf)
      sudo dnf install -y \
        gstreamer1-plugins-base \
        gstreamer1-plugins-good \
        gstreamer1-plugins-bad-free \
        gstreamer1-plugins-bad-freeworld || true \
        gstreamer1 \
        pipewire pipewire-alsa pipewire-pulseaudio wireplumber \
        pulseaudio-utils
      ;;
    pacman)
      sudo pacman -Sy --noconfirm \
        gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad \
        pipewire pipewire-alsa pipewire-pulse wireplumber \
        pulseaudio-alsa
      ;;
    *)
      echo "⚠️  Please install GStreamer (base/good/bad) and PipeWire manually."
      ;;
  esac

  # Make sure PulseAudio compatibility (pipewire-pulse) is running
  systemctl --user enable --now pipewire pipewire-pulse wireplumber 2>/dev/null || true
}

pactl_or_pw() {
  if need_cmd pactl; then echo "pactl"; else echo ""; fi
}

create_bridge_sink() {
  local ctl
  ctl="$(pactl_or_pw)"
  if [[ -z "$ctl" ]]; then
    echo "⚠️  'pactl' not found. Install pulseaudio-utils or pipewire-pulse."
    exit 1
  fi

  echo "→ Creating null sink 'bridge_out' (if not present)…"
  # If module is already loaded, command will fail; that's OK.
  $ctl load-module module-null-sink sink_name=bridge_out sink_properties=device.description=bridge_out >/dev/null 2>&1 || true

  echo "→ Current sinks:"
  $ctl list short sinks || true

  cat <<'EOF'

Routing plan:
  - Set default sink → bridge_out
  - Move any app playback to 'bridge_out' (so you still hear via monitor if you wish)
  - Capture from 'bridge_out.monitor' in the sender (pipewiresrc).

Helpful tools:
  - qpwgraph (graph GUI), pavucontrol (legacy Pulse UI)
EOF
}

remove_bridge_sink() {
  local ctl module_id
  ctl="$(pactl_or_pw)"
  echo "→ Attempting to unload module-null-sink named 'bridge_out'…"
  # Find module id for 'module-null-sink' with sink_name=bridge_out
  module_id="$($ctl list modules 2>/dev/null | awk '
    BEGIN{m=0}
    /Module #/{id=$2}
    /Name: module-null-sink/{m=1}
    /Argument: /{ if (m && $0 ~ /sink_name=bridge_out/) print id; m=0}
  ' | head -n1 || true)"
  if [[ -n "${module_id:-}" ]]; then
    $ctl unload-module "$module_id" || true
    echo "✓ Unloaded module $module_id."
  else
    echo "No module-null-sink bridge_out found."
  fi
}

list_sinks() {
  local ctl; ctl="$(pactl_or_pw)"
  $ctl list short sinks || true
}

list_sources() {
  local ctl; ctl="$(pactl_or_pw)"
  $ctl list short sources || true
}

set_default_sink() {
  local ctl; ctl="$(pactl_or_pw)"
  local sink="${1:-bridge_out}"
  echo "→ Setting default sink to: $sink"
  $ctl set-default-sink "$sink"
}

move_all_playback_to() {
  local ctl; ctl="$(pactl_or_pw)"
  local sink="${1:-bridge_out}"
  echo "→ Moving all playback streams to sink: $sink"
  # Move all current sink-inputs (playback) to the chosen sink
  for input in $($ctl list short sink-inputs | awk '{print $1}'); do
    $ctl move-sink-input "$input" "$sink" || true
  done
}

show_monitor_hint() {
  cat <<'EOF'

Find the monitor source (capture this for TX):
  ./linux_setup.sh list_sources | grep bridge_out.monitor

Sender caps (Opus over RTP) will use pipewiresrc. In v1, our Rust daemon
selects default; if multiple sources exist, you can set the default in your
session via qpwgraph/pavucontrol, or we can add a CLI flag in v1.1.

Quick TX/RX test (reverse of macOS example):

# On Linux (sender):
gst-launch-1.0 -v \
  pipewiresrc ! audioconvert ! audioresample \
  ! opusenc frame-size=2.5 bitrate=256000 \
  ! rtpopuspay pt=97 \
  ! udpsink host=<MAC_IP> port=5002

# On macOS (receiver):
gst-launch-1.0 -v \
  udpsrc port=5002 caps="application/x-rtp,media=audio,encoding-name=OPUS,clock-rate=48000,pt=97" \
  ! rtpjitterbuffer latency=10 drop-on-late=true do-lost=true \
  ! rtpopusdepay ! opusdec ! audioconvert ! audioresample ! osxaudiosink

EOF
}

test_rx() {
  cat <<'EOF'

Minimal receiver (this box):
  gst-launch-1.0 -v \
    udpsrc port=5004 caps="application/x-rtp,media=audio,encoding-name=OPUS,clock-rate=48000,pt=97" \
    ! rtpjitterbuffer latency=10 drop-on-late=true do-lost=true \
    ! rtpopusdepay ! opusdec ! audioconvert ! audioresample ! pipewiresink

EOF
}

main() {
  print_header
  if [[ "$(uname -s)" != "Linux" ]]; then
    echo "This script is for Linux only."
    exit 1
  fi

  case "${1:-setup}" in
    setup)
      install_pkgs
      create_bridge_sink
      set_default_sink bridge_out
      move_all_playback_to bridge_out
      list_sinks
      list_sources
      show_monitor_hint
      ;;
    create_bridge_sink)
      create_bridge_sink
      ;;
    remove_bridge_sink)
      remove_bridge_sink
      ;;
    list_sinks)
      list_sinks
      ;;
    list_sources)
      list_sources
      ;;
    set_default_sink)
      shift
      set_default_sink "${1:-bridge_out}"
      ;;
    move_all_playback_to)
      shift
      move_all_playback_to "${1:-bridge_out}"
      ;;
    test_rx)
      test_rx
      ;;
    *)
      echo "Commands:"
      echo "  setup (default)"
      echo "  create_bridge_sink | remove_bridge_sink | list_sinks | list_sources"
      echo "  set_default_sink [NAME] | move_all_playback_to [NAME] | test_rx"
      exit 1
      ;;
  esac
}

main "$@"
