// Stub: In v2 read rtpjitterbuffer stats or queue level and adjust an
// upstream resampler ratio ±0.1% to keep target fill. With GStreamer,
// you can insert "audioresample" with dynamic "rate" (via caps renegotiation)
// or implement tiny rate pulls by periodic drop/dupe of 1 sample per N seconds.
