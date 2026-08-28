//! End-to-end test of the public FFI surface against a real (synthetic)
//! `.osm.pbf` file: ingest → exposure scoring → routing.
//!
//! The fixture (regenerate with `scripts/make_test_fixture.py`) is a 2×5
//! street grid with a dome camera on the short southern route, plus guard and
//! ALPR nodes that ingest must drop. See the script for the exact layout.

use schattenweg_core::{LatLon, Router};

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
