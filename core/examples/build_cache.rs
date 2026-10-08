//! Pre-generate the scored-graph cache at asset-build time, so the app's first
//! launch skips the exposure pass instead of paying it once on-device.
//!
//!     cargo run --release --example build_cache <routing.osm.pbf> <out.graphcache>
//!
//! Runs the same code path as the app (`Router::open`), so the output is
//! exactly what the app would have written itself. The cache is keyed to the
//! extract's contents: ship it beside that exact `.pbf`.

use schattenweg_core::Router;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(pbf), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: build_cache <routing.osm.pbf> <out.graphcache>");
        std::process::exit(2);
    };
    // Drop any stale file so `open` is forced down the build-and-write path.
    let _ = std::fs::remove_file(&out);

    let t = Instant::now();
    let router = Router::open(pbf.clone(), out.clone()).unwrap_or_else(|e| {
        eprintln!("failed to load {pbf}: {e}");
        std::process::exit(1);
    });
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        eprintln!("cache was not written to {out}");
        std::process::exit(1);
    }
    println!(
        "wrote {out} ({:.1} MB): {} cameras, {} places, {:.1?}",
        size as f64 / 1e6,
        router.camera_count(),
        router.place_count(),
        t.elapsed()
    );
}
