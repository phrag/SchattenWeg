//! Terminal demo of the whole pipeline: load an extract, then plan the same
//! walk at λ=0 (shortest) and λ=4 (camera-shy) and show the trade-off.
//!
//!     cargo run --release --example plan_route [extract.osm.pbf] [from] [to]
//!
//! `from`/`to` are `lat,lon`, defaulting to Alexanderplatz → Kottbusser Tor.
//! The extract defaults to the snapshot from scripts/build_map_assets.sh.
//! To exercise it against the test fixture instead:
//!
//!     cargo run --release --example plan_route -- \
//!         tests/fixtures/mini_berlin.osm.pbf 52.5200,13.4000 52.5200,13.4040

use schattenweg_core::{LatLon, Router};
use std::time::Instant;

fn parse_point(s: &str, what: &str) -> LatLon {
    let (lat, lon) = s
        .split_once(',')
        .unwrap_or_else(|| panic!("{what} must look like 52.52,13.40"));
    LatLon {
        lat: lat.trim().parse().expect("bad latitude"),
        lon: lon.trim().parse().expect("bad longitude"),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| "../data/berlin-routing.osm.pbf".to_string());
    // Alexanderplatz → Kottbusser Tor, a walk with plenty of watched streets.
    let start = args
        .next()
        .map(|s| parse_point(&s, "from"))
        .unwrap_or(LatLon {
            lat: 52.5216,
            lon: 13.4127,
        });
    let end = args
        .next()
        .map(|s| parse_point(&s, "to"))
        .unwrap_or(LatLon {
            lat: 52.4991,
            lon: 13.4179,
        });

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

    // Sweep the paranoia dial so the trade-off is visible: length should climb
    // as mean exposure falls, with the route flipping at some crossover.
    println!(
        "{:>5}  {:>9}  {:>10}  {:>7}",
        "λ", "length", "exposure", "plan"
    );
    for lambda in [0.0, 1.0, 2.0, 4.0, 8.0] {
        let t = Instant::now();
        match router.plan(start, end, lambda) {
            Ok(route) => println!(
                "{lambda:>5}  {:>7.0} m  {:>9.1}%  {:>7.1?}",
                route.length_m,
                route.mean_exposure * 100.0,
                t.elapsed()
            ),
            Err(e) => println!("{lambda:>5}  {e}"),
        }
    }
}
