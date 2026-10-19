//! linger: a fast, native Bible reader.
//!
//! Environment:
//! - `LINGER_HOME` — data directory override (study database, imports, caches)
//! - `LINGER_RENDERER=glow|wgpu` — force a renderer (default: wgpu/Metal on macOS, glow elsewhere)
//! - `LINGER_STARTUP_PROBE=1` — print startup timings as JSON after the first frame and exit
//! - `LINGER_SCROLL_PROBE=1` — scroll through the Bible programmatically, print frame timings, exit
//! - `LINGER_SIDEBAR_PROBE=<n>` — open and close the sidebar n times, print frame timings, exit
//! - `LINGER_BACKGROUND=1` — macOS: open the window without taking focus (screenshots, benchmarks)

fn main() -> eframe::Result {
    linger::run()
}
