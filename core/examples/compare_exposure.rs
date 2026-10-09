//! Does hiding walls change route exposure? Plans the same random shortest
//! walks on two extracts that differ only in whether they contain buildings,
//! and reports how the mean exposure of those walks compares.
//!
//!     cargo run --release --example compare_exposure -- \
//!         with-buildings.osm.pbf no-buildings.osm.pbf [pairs] [south west north east]
//!
//! Both extracts must have the same streets and cameras (build the second by
//! filtering the first without `w/building r/building`, as in
//! `scripts/build_map_assets.sh`). Walks are planned at λ=0, so the path is the
//! shortest one on both and only the exposure of that path can differ. Points
//! are drawn from a bounding box (default: central Berlin) with a fixed seed,
//! so a run is repeatable.
//!
//! Buildings can only *remove* coverage, so no walk may come out *more* exposed
//! with buildings than without; any such pair is printed and is a bug (or a
//! tie between two equally short paths, which the length check filters out).

use schattenweg_core::{LatLon, Router};

/// Tiny deterministic generator (SplitMix64): no dependency, repeatable runs.
struct Rng(u64);

impl Rng {
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn load(path: &str) -> std::sync::Arc<Router> {
    Router::from_pbf(path.to_string()).unwrap_or_else(|e| {
        eprintln!("failed to load {path}: {e}");
        std::process::exit(1);
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: compare_exposure <with.osm.pbf> <without.osm.pbf> [pairs] [south west north east]");
        std::process::exit(2);
    }
    let pairs: usize = args
        .get(2)
        .map_or(2000, |s| s.parse().expect("bad pair count"));
    let bbox: Vec<f64> = if args.len() >= 7 {
        args[3..7]
            .iter()
            .map(|s| s.parse().expect("bad bbox"))
            .collect()
    } else {
        // Mitte and its neighbours, where the cameras are densest.
        vec![52.500, 13.350, 52.540, 13.440]
    };
    let (south, west, north, east) = (bbox[0], bbox[1], bbox[2], bbox[3]);

    let with = load(&args[0]);
    let without = load(&args[1]);
    println!(
        "cameras: {} with / {} without buildings",
        with.camera_count(),
        without.camera_count()
    );

    let mut rng = Rng(0x5348_4144_4f57);
    let mut point = || LatLon {
        lat: south + (north - south) * rng.next_f64(),
        lon: west + (east - west) * rng.next_f64(),
    };

    let (mut planned, mut lower, mut higher, mut skipped) = (0usize, 0usize, 0usize, 0usize);
    let (mut sum_with, mut sum_without, mut best_drop) = (0.0_f64, 0.0_f64, 0.0_f64);
    let mut best: Option<(LatLon, LatLon, f64, f64)> = None;
    for _ in 0..pairs {
        let (a, b) = (point(), point());
        let (Ok(rw), Ok(ro)) = (with.plan(a, b, 0.0), without.plan(a, b, 0.0)) else {
            skipped += 1;
            continue;
        };
        // Different lengths means two equally good paths were broken
        // differently, not a building effect.
        if (rw.length_m - ro.length_m).abs() > 0.01 {
            skipped += 1;
            continue;
        }
        planned += 1;
        sum_with += rw.mean_exposure;
        sum_without += ro.mean_exposure;
        let drop = ro.mean_exposure - rw.mean_exposure;
        if drop > 1e-6 {
            lower += 1;
            if drop > best_drop {
                best_drop = drop;
                best = Some((a, b, ro.mean_exposure, rw.mean_exposure));
            }
        } else if drop < -1e-6 {
            higher += 1;
            println!(
                "MORE exposed with buildings (bug?): {:.5},{:.5} -> {:.5},{:.5}: {:.1}% vs {:.1}%",
                a.lat,
                a.lon,
                b.lat,
                b.lon,
                rw.mean_exposure * 100.0,
                ro.mean_exposure * 100.0
            );
        }
    }

    let n = planned.max(1) as f64;
    println!("compared {planned} walks ({skipped} skipped: unroutable or tied paths)");
    println!(
        "mean exposure: {:.2}% with buildings, {:.2}% without",
        sum_with / n * 100.0,
        sum_without / n * 100.0
    );
    println!(
        "{lower} walks less exposed with buildings, {higher} more exposed, {} identical",
        planned - lower - higher
    );
    if let Some((a, b, without_pct, with_pct)) = best {
        println!(
            "biggest drop: {:.1}% -> {:.1}%  ({:.5},{:.5} -> {:.5},{:.5})",
            without_pct * 100.0,
            with_pct * 100.0,
            a.lat,
            a.lon,
            b.lat,
            b.lon
        );
    }
}
