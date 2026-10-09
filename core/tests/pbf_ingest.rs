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

// ---------------------------------------------------------------------------
// Buildings blocking camera coverage.
//
// Fixture: `mini_berlin_buildings.osm.pbf` (regenerate with
// `scripts/make_building_fixture.py`). Scenes 0-2 are an 80 m footway with a
// dome camera 12 m north of its middle and a block between them; see the script
// for what each block is. Scene k sits 0.01° of longitude east of scene k-1.
// ---------------------------------------------------------------------------

fn building_fixture() -> String {
    format!(
        "{}/tests/fixtures/mini_berlin_buildings.osm.pbf",
        env!("CARGO_MANIFEST_DIR")
    )
}

const M_PER_DEG_LAT: f64 = 111_195.0;

/// (lat, lon) of a point `east_m`/`north_m` from the origin of `scene`.
fn scene_point(scene: u32, east_m: f64, north_m: f64) -> LatLon {
    let m_per_deg_lon = M_PER_DEG_LAT * 52.52_f64.to_radians().cos();
    LatLon {
        lat: 52.5200 + north_m / M_PER_DEG_LAT,
        lon: 13.4000 + f64::from(scene) * 0.01 + east_m / m_per_deg_lon,
    }
}

/// Mean exposure walking the length of a scene's street.
fn street_exposure(router: &Router, scene: u32) -> f64 {
    router
        .plan(
            scene_point(scene, -40.0, 0.0),
            scene_point(scene, 40.0, 0.0),
            0.0,
        )
        .expect("street route")
        .mean_exposure
}

/// Distance in metres from the camera to ring point `i` of its coverage disc.
fn ring_reach_m(router: &Router, scene: u32, i: usize) -> f64 {
    let cam = scene_point(scene, 0.0, 12.0);
    let shapes = router.coverage_near(cam, 5.0);
    assert_eq!(shapes.len(), 1, "one camera in scene {scene}");
    let p = shapes[0].ring[i];
    let m_per_deg_lon = M_PER_DEG_LAT * 52.52_f64.to_radians().cos();
    let dy = (p.lat - cam.lat) * M_PER_DEG_LAT;
    let dx = (p.lon - cam.lon) * m_per_deg_lon;
    dx.hypot(dy)
}

#[test]
fn a_tall_building_hides_the_street_from_the_camera() {
    let router = Router::from_pbf(building_fixture()).expect("fixture should load");
    assert_eq!(router.camera_count(), 3);
    assert!(
        street_exposure(&router, 0) < 0.01,
        "wall: {}",
        street_exposure(&router, 0)
    );
}

#[test]
fn a_garage_does_not_hide_anything() {
    let router = Router::from_pbf(building_fixture()).expect("fixture should load");
    let e = street_exposure(&router, 1);
    assert!(e > 0.25, "garage scene should stay watched, got {e}");
}

#[test]
fn a_multipolygon_building_relation_blocks_too() {
    let router = Router::from_pbf(building_fixture()).expect("fixture should load");
    assert!(
        street_exposure(&router, 2) < 0.01,
        "relation: {}",
        street_exposure(&router, 2)
    );
}

#[test]
fn coverage_outline_is_cut_by_the_wall_but_not_the_garage() {
    let router = Router::from_pbf(building_fixture()).expect("fixture should load");
    // The disc ring has 72 rays, ring[0] due north and ring[36] due south, then
    // a closing point.
    assert_eq!(
        router.coverage_near(scene_point(0, 0.0, 12.0), 5.0)[0]
            .ring
            .len(),
        73
    );
    // North of the camera there is nothing: full 20 m range in every scene.
    for scene in 0..3 {
        let north = ring_reach_m(&router, scene, 0);
        assert!((north - 20.0).abs() < 0.5, "scene {scene} north {north}");
    }
    // South, the block's far wall is 4 m away (12 m - 8 m).
    for scene in [0, 2] {
        let south = ring_reach_m(&router, scene, 36);
        assert!((south - 4.0).abs() < 0.5, "scene {scene} south {south}");
    }
    let south = ring_reach_m(&router, 1, 36);
    assert!((south - 20.0).abs() < 0.5, "garage south {south}");
}

#[test]
fn cached_router_keeps_the_buildings() {
    let cache = TempCache::new("buildings");
    let fresh = Router::open(building_fixture(), cache.path()).expect("first open builds");
    let cached = Router::open(building_fixture(), cache.path()).expect("second open reads cache");
    for scene in 0..3 {
        assert_eq!(
            street_exposure(&fresh, scene),
            street_exposure(&cached, scene),
            "scene {scene}"
        );
        assert_eq!(
            ring_reach_m(&fresh, scene, 36),
            ring_reach_m(&cached, scene, 36),
            "scene {scene}"
        );
    }
    // And it really is the cached one that is blocking, not a rebuild.
    assert!(street_exposure(&cached, 0) < 0.01);
}

#[test]
fn a_fixture_without_buildings_is_unchanged() {
    // The original fixture has no building ways: scoring must be exactly the
    // old range-and-bearing behaviour.
    let router = Router::from_pbf(fixture()).expect("fixture should load");
    let start = LatLon {
        lat: 52.5200,
        lon: 13.4000,
    };
    let end = LatLon {
        lat: 52.5200,
        lon: 13.4040,
    };
    assert!(router.plan(start, end, 0.0).unwrap().mean_exposure > 0.05);
}
