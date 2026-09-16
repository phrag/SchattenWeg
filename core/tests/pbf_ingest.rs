//! End-to-end test of the public FFI surface against a real (synthetic)
//! `.osm.pbf` file: ingest → exposure scoring → routing.
//!
//! The fixture (regenerate with `scripts/make_test_fixture.py`) is a 2×5
//! street grid with a dome camera on the short southern route, plus guard and
//! ALPR nodes that ingest must drop. See the script for the exact layout.

use schattenweg_core::{LatLon, PlaceKind, Router};

fn fixture() -> String {
    format!(
        "{}/tests/fixtures/mini_berlin.osm.pbf",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn loads_only_actual_cameras() {
    let router = Router::from_pbf(fixture()).expect("fixture should load");
    // Dome + fixed; the guard and ALPR nodes must be dropped.
    assert_eq!(router.camera_count(), 2);
}

#[test]
fn cameras_near_filters_by_radius() {
    let router = Router::from_pbf(fixture()).expect("fixture should load");
    let at = LatLon {
        lat: 52.5200,
        lon: 13.4015,
    };
    // Only the dome is within 100 m; the fixed camera sits ~200 m away.
    assert_eq!(router.cameras_near(at, 100.0).len(), 1);
    assert_eq!(router.cameras_near(at, 500.0).len(), 2);
}

#[test]
fn lambda_trades_length_for_exposure() {
    let router = Router::from_pbf(fixture()).expect("fixture should load");
    let start = LatLon {
        lat: 52.5200,
        lon: 13.4000,
    };
    let end = LatLon {
        lat: 52.5200,
        lon: 13.4040,
    };

    let direct = router.plan(start, end, 0.0).expect("direct route");
    let shy = router.plan(start, end, 8.0).expect("camera-shy route");

    // λ=0 walks straight past the camera: ~272 m southern row, watched.
    assert!(
        (250.0..300.0).contains(&direct.length_m),
        "direct length {}",
        direct.length_m
    );
    assert!(
        direct.mean_exposure > 0.05,
        "direct exposure {}",
        direct.mean_exposure
    );

    // λ=8 detours via the northern row: longer, but out of every lens.
    assert!(
        shy.length_m > direct.length_m + 100.0,
        "shy length {}",
        shy.length_m
    );
    assert!(
        shy.mean_exposure < 0.01,
        "shy exposure {}",
        shy.mean_exposure
    );
}

#[test]
fn far_away_points_are_refused() {
    let router = Router::from_pbf(fixture()).expect("fixture should load");
    let start = LatLon {
        lat: 52.5200,
        lon: 13.4000,
    };
    let far = LatLon {
        lat: 48.1,
        lon: 11.6,
    }; // Munich
    assert!(router.plan(start, far, 0.0).is_err());
}

#[test]
fn indexes_streets_localities_and_stations() {
    let router = Router::from_pbf(fixture()).expect("fixture should load");
    // Two streets (one split across two ways), one quarter, one station.
    // The cafe must not be indexed.
    assert_eq!(router.place_count(), 4);
}

#[test]
fn finds_a_street_by_partial_name() {
    let router = Router::from_pbf(fixture()).expect("fixture should load");
    let hits = router.search_places("kamera".to_string(), 10);
    assert_eq!(hits.len(), 1, "a split street must collapse to one result");
    assert_eq!(hits[0].name, "Kameraweg");
    assert_eq!(hits[0].kind, PlaceKind::Street);
    // Resolved to a real coordinate on the street, not 0,0.
    assert!((hits[0].lat - 52.52).abs() < 0.01);
}

#[test]
fn finds_a_station_and_a_locality() {
    let router = Router::from_pbf(fixture()).expect("fixture should load");
    assert_eq!(
        router.search_places("bahnhof".to_string(), 5)[0].kind,
        PlaceKind::Station
    );
    assert_eq!(
        router.search_places("mitte".to_string(), 5)[0].kind,
        PlaceKind::Locality
    );
}

#[test]
fn does_not_index_arbitrary_pois() {
    let router = Router::from_pbf(fixture()).expect("fixture should load");
    assert!(router.search_places("cafe".to_string(), 5).is_empty());
}

/// A per-test scratch cache path that is cleaned up on drop, so a failing test
/// never leaves a file behind to poison the next run.
struct TempCache(std::path::PathBuf);
impl TempCache {
    fn new(tag: &str) -> Self {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "schattenweg-cache-test-{tag}-{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        Self(p)
    }
    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}
impl Drop for TempCache {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn open_writes_a_cache_then_reads_it_back_identically() {
    let cache = TempCache::new("roundtrip");

    // First open: cache miss, builds from the PBF and writes the cache.
    let fresh = Router::open(fixture(), cache.path()).expect("first open builds");
    assert!(
        std::path::Path::new(&cache.path()).exists(),
        "open should have written a cache file"
    );

    // Second open: cache hit, must yield an identical router.
    let cached = Router::open(fixture(), cache.path()).expect("second open reads cache");

    assert_eq!(fresh.camera_count(), cached.camera_count());
    assert_eq!(fresh.place_count(), cached.place_count());

    let start = LatLon {
        lat: 52.5200,
        lon: 13.4000,
    };
    let end = LatLon {
        lat: 52.5200,
        lon: 13.4040,
    };
    for lambda in [0.0, 8.0] {
        let a = fresh.plan(start, end, lambda).expect("fresh route");
        let b = cached.plan(start, end, lambda).expect("cached route");
        assert_eq!(a.length_m, b.length_m, "length differs at λ={lambda}");
        assert_eq!(
            a.mean_exposure, b.mean_exposure,
            "exposure differs at λ={lambda}"
        );
        assert_eq!(
            a.polyline.len(),
            b.polyline.len(),
            "path differs at λ={lambda}"
        );
    }
}

#[test]
fn open_matches_from_pbf() {
    let cache = TempCache::new("matches-pbf");
    let direct = Router::from_pbf(fixture()).expect("from_pbf");
    let opened = Router::open(fixture(), cache.path()).expect("open");
    assert_eq!(direct.camera_count(), opened.camera_count());
    assert_eq!(direct.place_count(), opened.place_count());
}

#[test]
fn open_falls_back_when_cache_is_corrupt() {
    let cache = TempCache::new("corrupt");
    // A garbage file where the cache should be: open must ignore it, rebuild
    // from the PBF, and overwrite it with a valid cache.
    std::fs::write(&cache.0, b"not a real cache").expect("seed corrupt file");
    let router = Router::open(fixture(), cache.path()).expect("open recovers from junk");
    assert_eq!(router.camera_count(), 2);
    // The corrupt file was replaced, so a second open now hits the cache.
    let again = Router::open(fixture(), cache.path()).expect("second open");
    assert_eq!(again.camera_count(), 2);
}
