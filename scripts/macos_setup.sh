#!/usr/bin/env bash
# macos_setup.sh — v1 bootstrap for the Rust LAN audio bridge (macOS)
# - Installs core deps: GStreamer, BlackHole (2ch), SwitchAudioSource (CLI)
# - Prints step-by-step to create a Multi-Output Device (speakers + BlackHole)
# - Provides convenience helpers to route system audio for TX tests
#
# Usage:
#   chmod +x macos_setup.sh
#   ./macos_setup.sh
#
# Afterwards:
#   ./macos_setup.sh route_system_to_multi     # (after you create it)
#   ./macos_setup.sh route_system_to_builtin
#   ./macos_setup.sh list_outputs

set -euo pipefail

print_header() {
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "  macOS Audio Bridge Setup (v1)"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || return 1
}

install_homebrew_if_missing() {
  if ! need_cmd brew; then
    echo "Homebrew is required. Install from https://brew.sh and re-run."
    exit 1
  fi
}

install_pkgs() {
  echo "→ Installing packages with Homebrew…"
  brew update

  # GStreamer + plugins (base/good/bad)
  brew list gstreamer >/dev/null 2>&1 || brew install gstreamer
  for p in gst-plugins-base gst-plugins-good gst-plugins-bad; do
    brew list "$p" >/dev/null 2>&1 || brew install "$p"
  done

  # BlackHole 2ch (virtual audio device)
  if ! system_profiler SPAudioDataType 2>/dev/null | grep -qi "BlackHole 2ch"; then
    # cask is the usual route
    if brew info --cask blackhole-2ch >/dev/null 2>&1; then
      brew install --cask blackhole-2ch
    else
      echo "⚠️  Could not find Homebrew cask 'blackhole-2ch'."
      echo "    Download from: https://existential.audio/blackhole/ (2ch)"
      echo "    Install it, then re-run this script."
      exit 1
    fi
  else
    echo "✓ BlackHole 2ch already installed."
  fi

  # SwitchAudioSource CLI (to list/select outputs)
  brew list switchaudio-osx >/dev/null 2>&1 || brew install switchaudio-osx
}

show_multi_output_instructions() {
  cat <<'EOF'

────────────────────────────────────────────────────────────────
Create a Multi-Output Device (once, manual step)
────────────────────────────────────────────────────────────────
1) Open “Audio MIDI Setup” (launching now).
2) Bottom-left “+” → “Create Multi-Output Device”.
3) On the right, CHECK:
     [x] BlackHole 2ch
     [x] (Your speakers) e.g., MacBook Speakers or Headphones
4) (Optional) Rename it to:  AB Multi-Output
5) Right-click the new device → “Use This Device For Sound Output”
   (You can switch back later.)

This mirrors ALL system audio to:
  • your actual speakers AND
  • BlackHole (which our sender captures).

You can switch system output at will in System Settings → Sound.
EOF

  # Try to open the Audio MIDI Setup app for convenience
  open -a "Audio MIDI Setup" || true
}

list_outputs() {
  echo "Available OUTPUT devices:"
  SwitchAudioSource -a -t output | sed 's/^/  • /'
}

route_system_to_multi() {
  local target="${1:-AB Multi-Output}"
  echo "→ Setting system output to: $target"
  if ! SwitchAudioSource -s "$target" -t output; then
    echo "⚠️  Could not select '$target'. Is the Multi-Output Device created?"
    echo "    Use: ./macos_setup.sh list_outputs"
    exit 1
  fi
  echo "✓ System output set to '$target'."
}

route_system_to_builtin() {
  # Common names: "MacBook Pro Speakers", "MacBook Speakers", "Built-in Output"
  # We'll try a few:
  local candidates=("MacBook Pro Speakers" "MacBook Speakers" "Built-in Output" "External Headphones")
  for c in "${candidates[@]}"; do
    if SwitchAudioSource -a -t output | grep -qx "$c"; then
      echo "→ Setting system output to: $c"
      SwitchAudioSource -s "$c" -t output && { echo "✓ Done."; return 0; }
    fi
  done
  echo "⚠️  Could not find a built-in output automatically. Use:"
  echo "    ./macos_setup.sh list_outputs"
  echo "    ./macos_setup.sh route_system_to_multi \"<Exact Device Name>\""
  exit 1
}

test_tx_hint() {
  cat <<'EOF'

Quick TX/RX test (replace <LINUX_IP>):

# On Linux (receiver):
gst-launch-1.0 -v \
  udpsrc port=5004 caps="application/x-rtp,media=audio,encoding-name=OPUS,clock-rate=48000,pt=97" \
  ! rtpjitterbuffer latency=10 drop-on-late=true do-lost=true \
  ! rtpopusdepay ! opusdec ! audioconvert ! audioresample ! pipewiresink

# On macOS (sender), after routing system audio to the Multi-Output:
gst-launch-1.0 -v \
  osxaudiosrc device="BlackHole 2ch" buffer-time=5000 latency-time=5000 \
  ! audioconvert ! audioresample \
  ! opusenc frame-size=2.5 bitrate=256000 \
  ! rtpopuspay pt=97 \
  ! udpsink host=<LINUX_IP> port=5004

EOF
}

main() {
  print_header
  if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "This script is for macOS only."
    exit 1
  fi
  install_homebrew_if_missing

  case "${1:-setup}" in
    setup)
      install_pkgs
      show_multi_output_instructions
      list_outputs
      test_tx_hint
      ;;
    list_outputs)
      list_outputs
      ;;
    route_system_to_multi)
      shift
      route_system_to_multi "${1:-AB Multi-Output}"
      ;;
    route_system_to_builtin)
      route_system_to_builtin
      ;;
    *)
      echo "Unknown command: $1"
      echo "Commands: setup (default) | list_outputs | route_system_to_multi [NAME] | route_system_to_builtin"
      exit 1
      ;;
  esac
}

main "$@"
