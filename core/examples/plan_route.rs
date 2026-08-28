//! Terminal demo of the whole pipeline: load a Berlin extract, then plan the
//! same walk at λ=0 (shortest) and λ=4 (camera-shy) and show the trade-off.
//!
//!     cargo run --release --example plan_route [path/to/berlin.osm.pbf]
//!
//! Defaults to the snapshot produced by scripts/build_map_assets.sh.

use schattenweg_core::{LatLon, Router};
use std::time::Instant;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "../data/berlin-routing.osm.pbf".to_string());

    let t0 = Instant::now();
    let router = match Router::from_pbf(path.clone()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("failed to load {path}: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "loaded {} cameras from {path} in {:.1?}",
        router.camera_count(),
        t0.elapsed()
    );

    // Alexanderplatz → Kottbusser Tor, a walk with plenty of watched streets.
    let start = LatLon {
        lat: 52.5216,
        lon: 13.4127,
    };
    let end = LatLon {
        lat: 52.4991,
        lon: 13.4179,
    };

    for lambda in [0.0, 4.0] {
        let t = Instant::now();
        match router.plan(start, end, lambda) {
            Ok(route) => println!(
                "λ={lambda}: {:.0} m, mean exposure {:.1}%, {} points, planned in {:.1?}",
                route.length_m,
                route.mean_exposure * 100.0,
                route.polyline.len(),
                t.elapsed()
            ),
            Err(e) => println!("λ={lambda}: {e}"),
        }
    }
}
