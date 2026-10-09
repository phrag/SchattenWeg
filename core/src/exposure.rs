//! Per-edge surveillance exposure scoring.
//!
//! This is the module the whole app lives or dies on. Ordinary routing
//! minimises distance/time; we add a per-edge **exposure score** in [0, 1]
//! that says "what fraction of this segment is inside at least one camera's
//! coverage". The router then trades that off against length via a single
//! paranoia parameter λ (see `routing.rs`).
//!
//! The score is computed by walking each edge in small steps and asking, at
//! each sample point, whether it is watched: covered by a nearby camera, or
//! inside a police AI-video zone (see `zones.rs`). Cameras are held in a
//! spatial index so "nearby" is cheap; the naive all-pairs version is left in
//! a comment for clarity.

use crate::camera::{haversine_m, Camera};
use crate::occluders::OccluderIndex;
use crate::zones::ZoneIndex;
use std::collections::HashMap;

/// A node in the routing graph — just a coordinate plus its stable id.
#[derive(Debug, Clone, Copy, uniffi::Record)]
pub struct Node {
    pub id: u64,
    pub lat: f64,
    pub lon: f64,
}

/// A directed edge between two nodes. `exposure` is filled in by
/// [`score_edges`] and starts at 0.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Edge {
    pub from: u64,
    pub to: u64,
    /// Segment length in metres (great-circle; fine for short OSM ways).
    pub length_m: f64,
    /// Fraction of the segment under surveillance, in [0, 1]. Filled by scoring.
    pub exposure: f64,
}

/// Distance between successive sample points along an edge, in metres.
/// Smaller = more accurate exposure, more compute. 5 m is a good default at
/// city scale given typical camera ranges of 20–30 m.
const SAMPLE_STEP_M: f64 = 5.0;

/// Hard ceiling on samples per edge, so untrusted input can't turn one
/// pathological edge into an unbounded loop. 2000 samples covers a 10 km edge
/// at full resolution — far longer than any real OSM street segment.
const MAX_SAMPLES: usize = 2_000;

/// Compute and attach an exposure score to every edge, given the node table,
/// the camera set and the AI-video zones. Mutates `edges` in place.
///
/// `nodes_lookup` must resolve a node id to its coordinates. In the real graph
/// this is a slice indexed by a compact id; here we take a closure so the
/// scoring logic stays independent of graph storage.
pub fn score_edges<F>(
    edges: &mut [Edge],
    cameras: &CameraIndex,
    zones: &ZoneIndex,
    mut node_coord: F,
) where
    F: FnMut(u64) -> (f64, f64),
{
    for edge in edges.iter_mut() {
        let (alat, alon) = node_coord(edge.from);
        let (blat, blon) = node_coord(edge.to);
        edge.exposure = edge_exposure(alat, alon, blat, blon, edge.length_m, cameras, zones);
    }
}

/// Fraction of the A→B segment that lies inside any camera's coverage or any
/// AI-video zone.
fn edge_exposure(
    alat: f64,
    alon: f64,
    blat: f64,
    blon: f64,
    length_m: f64,
    cameras: &CameraIndex,
    zones: &ZoneIndex,
) -> f64 {
    // NaN (from malformed coordinates) fails this test and is treated as
    // zero-length, same as a degenerate edge.
    if length_m.is_nan() || length_m <= f64::EPSILON {
        return 0.0;
    }
    // Sample count is bounded: a corrupt or hostile extract could otherwise
    // present a single edge thousands of kilometres long and stall the load
    // pass. Beyond the cap the sampling just gets coarser.
    let steps = (length_m / SAMPLE_STEP_M)
        .ceil()
        .max(1.0)
        .min(MAX_SAMPLES as f64) as usize;
    let mut covered = 0usize;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        // Linear interpolation in lat/lon is fine over a single OSM edge.
        let lat = alat + (blat - alat) * t;
        let lon = alon + (blon - alon) * t;
        if cameras.any_covers(lat, lon) || zones.contains(lat, lon) {
            covered += 1;
        }
    }
    covered as f64 / (steps as f64 + 1.0)
}

/// Metres per degree of latitude (and of longitude at the equator).
const M_PER_DEG_LAT: f64 = 111_320.0;

/// Spatial index over cameras so exposure scoring doesn't go quadratic
/// (~10⁶ edge samples × 10³ cameras in Berlin).
///
/// A deliberately dumb uniform grid: cameras are bucketed into lat/lon cells
/// sized to the maximum camera range, so any camera that could cover a query
/// point lives in the 3×3 block of cells around it. At Berlin's camera density
/// this is plenty; swap in an R-tree (`rstar`) later if a city ever gets
/// pathological.
///
/// It also carries the building outlines that block cameras' view (see
/// `occluders.rs`), so "covered" always means "in range, in the cone, **and**
/// not behind a wall". Without buildings (the default) it is the plain
/// range-and-bearing test.
pub struct CameraIndex {
    cameras: Vec<Camera>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    cell_lat_deg: f64,
    cell_lon_deg: f64,
    occluders: OccluderIndex,
}

impl CameraIndex {
    pub fn new(cameras: Vec<Camera>) -> Self {
        let max_range_m = cameras
            .iter()
            .map(|c| c.range_m)
            .fold(0.0_f64, f64::max)
            .max(1.0);
        // Longitude degrees shrink with latitude; size cells for the camera
        // set's own latitude band so a cell is never narrower than max range.
        let mean_lat = if cameras.is_empty() {
            0.0
        } else {
            cameras.iter().map(|c| c.lat).sum::<f64>() / cameras.len() as f64
        };
        let cell_lat_deg = max_range_m / M_PER_DEG_LAT;
        let cell_lon_deg = max_range_m / (M_PER_DEG_LAT * mean_lat.to_radians().cos().max(0.01));

        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, cam) in cameras.iter().enumerate() {
            let key = (
                (cam.lat / cell_lat_deg).floor() as i32,
                (cam.lon / cell_lon_deg).floor() as i32,
            );
            grid.entry(key).or_default().push(i as u32);
        }

        Self {
            cameras,
            grid,
            cell_lat_deg,
            cell_lon_deg,
            occluders: OccluderIndex::empty(),
        }
    }

    /// Let these buildings block the cameras' view.
    pub fn with_occluders(mut self, occluders: OccluderIndex) -> Self {
        self.occluders = occluders;
        self
    }

    pub fn len(&self) -> usize {
        self.cameras.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cameras.is_empty()
    }

    /// True if any camera covers this point. The hot inner call of the
    /// exposure pass: only the 3×3 grid neighbourhood is tested, and the
    /// (comparatively costly) wall test runs only for cameras that already
    /// cover the point by range and bearing.
    pub fn any_covers(&self, lat: f64, lon: f64) -> bool {
        let ci = (lat / self.cell_lat_deg).floor() as i32;
        let cj = (lon / self.cell_lon_deg).floor() as i32;
        for di in -1..=1 {
            for dj in -1..=1 {
                if let Some(bucket) = self.grid.get(&(ci + di, cj + dj)) {
                    if bucket.iter().any(|&i| {
                        let cam = &self.cameras[i as usize];
                        cam.covers(lat, lon)
                            && !self.occluders.blocks((cam.lat, cam.lon), (lat, lon))
                    }) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The coverage outline of every camera within `radius_m` of a point, with
    /// walls cut out — what the map draws. Same cameras as [`Self::near`].
    pub fn coverage_near(&self, lat: f64, lon: f64, radius_m: f64) -> Vec<(i64, Vec<(f64, f64)>)> {
        self.near(lat, lon, radius_m)
            .into_iter()
            .map(|cam| (cam.osm_id, self.occluders.coverage_ring(&cam)))
            .collect()
    }

    /// All cameras within `radius_m` of a point — handy for the map layer
    /// ("show cameras near me") and for debugging exposure.
    pub fn near(&self, lat: f64, lon: f64, radius_m: f64) -> Vec<Camera> {
        self.cameras
            .iter()
            .filter(|c| haversine_m(c.lat, c.lon, lat, lon) <= radius_m)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::CameraKind;

    fn dome_at(lat: f64, lon: f64, range: f64) -> Camera {
        Camera {
            osm_id: 0,
            lat,
            lon,
            kind: CameraKind::Dome,
            direction_deg: None,
            half_fov_deg: 30.0,
            range_m: range,
        }
    }

    #[test]
    fn edge_far_from_cameras_scores_zero() {
        let idx = CameraIndex::new(vec![dome_at(52.60, 13.50, 20.0)]);
        let e = edge_exposure(52.52, 13.40, 52.52, 13.41, 700.0, &idx, &ZoneIndex::empty());
        assert_eq!(e, 0.0);
    }

    #[test]
    fn absurd_edge_length_is_bounded_not_hung() {
        // A corrupt extract could claim a single edge spans the planet. The
        // sample count must stay capped instead of looping for minutes.
        let idx = CameraIndex::new(vec![dome_at(52.52, 13.40, 25.0)]);
        let e = edge_exposure(
            52.52,
            13.40,
            -33.9,
            151.2,
            16_000_000.0,
            &idx,
            &ZoneIndex::empty(),
        );
        assert!((0.0..=1.0).contains(&e));
    }

    #[test]
    fn grid_matches_linear_scan() {
        // A scatter of cameras around central Berlin; the grid must agree with
        // a brute-force scan everywhere, including cell boundaries.
        let mut cams = Vec::new();
        for k in 0..60u32 {
            let lat = 52.50 + f64::from(k % 10) * 0.0012;
            let lon = 13.38 + f64::from(k / 10) * 0.0018;
            cams.push(dome_at(lat, lon, 25.0));
        }
        let idx = CameraIndex::new(cams.clone());
        for i in 0..40 {
            for j in 0..40 {
                let lat = 52.499 + f64::from(i) * 0.0004;
                let lon = 13.379 + f64::from(j) * 0.0006;
                let linear = cams.iter().any(|c| c.covers(lat, lon));
                assert_eq!(idx.any_covers(lat, lon), linear, "at {lat},{lon}");
            }
        }
    }

    #[test]
    fn edge_through_camera_scores_positive() {
        // Camera sitting right on the midpoint of a short edge.
        let idx = CameraIndex::new(vec![dome_at(52.5200, 13.4050, 30.0)]);
        let e = edge_exposure(
            52.5195,
            13.4050,
            52.5205,
            13.4050,
            111.0,
            &idx,
            &ZoneIndex::empty(),
        );
        assert!(e > 0.0 && e <= 1.0);
    }

    /// A building 10..20 m north of (52.5200, 13.4050), 10 m wide.
    fn block_north_of_camera() -> OccluderIndex {
        let m_lat = 1.0 / 111_195.0;
        let m_lon = m_lat / 52.52_f64.to_radians().cos();
        let ring = |n: f64, e: f64| (52.5200 + n * m_lat, 13.4050 + e * m_lon);
        OccluderIndex::new(vec![vec![
            ring(10.0, -5.0),
            ring(20.0, -5.0),
            ring(20.0, 5.0),
            ring(10.0, 5.0),
        ]])
    }

    /// A 60 m east-west street 25 m north of a dome camera, with the building
    /// from `block_north_of_camera` standing between them.
    fn street_behind_the_block() -> (f64, f64, f64, f64, f64) {
        let m_lat = 1.0 / 111_195.0;
        let m_lon = m_lat / 52.52_f64.to_radians().cos();
        let lat = 52.5200 + 25.0 * m_lat;
        (
            lat,
            13.4050 - 30.0 * m_lon,
            lat,
            13.4050 + 30.0 * m_lon,
            60.0,
        )
    }

    #[test]
    fn a_building_between_camera_and_street_lowers_exposure() {
        let (alat, alon, blat, blon, len) = street_behind_the_block();
        let open = CameraIndex::new(vec![dome_at(52.5200, 13.4050, 30.0)]);
        let walled = CameraIndex::new(vec![dome_at(52.5200, 13.4050, 30.0)])
            .with_occluders(block_north_of_camera());
        let zones = ZoneIndex::empty();
        let before = edge_exposure(alat, alon, blat, blon, len, &open, &zones);
        let after = edge_exposure(alat, alon, blat, blon, len, &walled, &zones);
        assert!(before > 0.4, "open street is watched: {before}");
        assert!(after < before, "{after} should be below {before}");
        // The building hides the middle of the street, not the ends.
        assert!(after > 0.0, "the street ends are still in view: {after}");
    }

    #[test]
    fn buildings_off_the_sight_line_change_nothing() {
        let (alat, alon, blat, blon, len) = street_behind_the_block();
        let m_lat = 1.0 / 111_195.0;
        let far = OccluderIndex::new(vec![vec![
            (52.5200 - 100.0 * m_lat, 13.4000),
            (52.5200 - 100.0 * m_lat, 13.4001),
            (52.5200 - 110.0 * m_lat, 13.4001),
        ]]);
        let open = CameraIndex::new(vec![dome_at(52.5200, 13.4050, 30.0)]);
        let with_far = CameraIndex::new(vec![dome_at(52.5200, 13.4050, 30.0)]).with_occluders(far);
        let zones = ZoneIndex::empty();
        assert_eq!(
            edge_exposure(alat, alon, blat, blon, len, &open, &zones),
            edge_exposure(alat, alon, blat, blon, len, &with_far, &zones),
        );
    }

    #[test]
    fn walls_do_not_hide_an_ai_zone() {
        // Zones are policy circles, not lines of sight: a building inside one
        // must not lower the exposure it contributes.
        let (alat, alon, blat, blon, len) = street_behind_the_block();
        let cams = CameraIndex::new(Vec::new()).with_occluders(block_north_of_camera());
        let zone = zone_100m(52.5200, 13.4050);
        assert_eq!(
            edge_exposure(alat, alon, blat, blon, len, &cams, &zone),
            1.0
        );
    }

    #[test]
    fn coverage_near_returns_a_clipped_ring_per_camera() {
        let idx = CameraIndex::new(vec![dome_at(52.5200, 13.4050, 30.0)])
            .with_occluders(block_north_of_camera());
        let shapes = idx.coverage_near(52.5200, 13.4050, 50.0);
        assert_eq!(shapes.len(), 1);
        let (_, ring) = &shapes[0];
        // The disc ring starts due north. Unclipped it would reach 30 m; the
        // wall 10 m up stops it there.
        let due_north_m = (ring[0].0 - 52.5200) * 111_195.0;
        assert!((due_north_m - 10.0).abs() < 0.5, "{due_north_m}");
        assert!(idx.coverage_near(52.6, 13.5, 50.0).is_empty());
    }

    /// A one-zone index centred on a made-up point, radius 100 m.
    fn zone_100m(lat: f64, lon: f64) -> ZoneIndex {
        use crate::zones::{AiZone, ZoneStatus};
        ZoneIndex::new(&[AiZone {
            id: "t".into(),
            name: "t".into(),
            lat,
            lon,
            radius_m: 100.0,
            status: ZoneStatus::Planned,
            note: String::new(),
            detects: String::new(),
        }])
    }

    #[test]
    fn edge_wholly_inside_a_zone_is_fully_exposed_with_no_cameras() {
        let none = CameraIndex::new(Vec::new());
        let zone = zone_100m(52.5200, 13.4050);
        // ~22 m of street through the centre: every sample is inside.
        let e = edge_exposure(52.5199, 13.4050, 52.5201, 13.4050, 22.0, &none, &zone);
        assert_eq!(e, 1.0);
    }

    #[test]
    fn edge_crossing_a_zone_scores_the_share_inside() {
        let none = CameraIndex::new(Vec::new());
        // Centred on the edge's midpoint with radius 100 m, a ~600 m
        // north-south edge has about 200 m (a third) inside.
        let zone = zone_100m(52.5200, 13.4050);
        let half_deg = 300.0 / 111_320.0;
        let e = edge_exposure(
            52.5200 - half_deg,
            13.4050,
            52.5200 + half_deg,
            13.4050,
            600.0,
            &none,
            &zone,
        );
        assert!((e - 1.0 / 3.0).abs() < 0.03, "share inside was {e}");
    }

    #[test]
    fn zone_and_camera_overlap_is_not_double_counted() {
        // Exposure is a covered fraction of sample points, so a camera inside
        // a zone must not push it past 1.
        let cam = CameraIndex::new(vec![dome_at(52.5200, 13.4050, 30.0)]);
        let zone = zone_100m(52.5200, 13.4050);
        let e = edge_exposure(52.5199, 13.4050, 52.5201, 13.4050, 22.0, &cam, &zone);
        assert_eq!(e, 1.0);
    }
}
