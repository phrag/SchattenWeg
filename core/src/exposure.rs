//! Per-edge surveillance exposure scoring.
//!
//! This is the module the whole app lives or dies on. Ordinary routing
//! minimises distance/time; we add a per-edge **exposure score** in [0, 1]
//! that says "what fraction of this segment is inside at least one camera's
//! coverage". The router then trades that off against length via a single
//! paranoia parameter λ (see `routing.rs`).
//!
//! The score is computed by walking each edge in small steps and asking, at
//! each sample point, whether any nearby camera covers it. Cameras are held in
//! a spatial index so "nearby" is cheap; the naive all-pairs version is left
//! in a comment for clarity.

use crate::camera::{haversine_m, Camera};

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

/// Compute and attach an exposure score to every edge, given the node table
/// and the camera set. Mutates `edges` in place.
///
/// `nodes_lookup` must resolve a node id to its coordinates. In the real graph
/// this is a slice indexed by a compact id; here we take a closure so the
/// scoring logic stays independent of graph storage.
pub fn score_edges<F>(edges: &mut [Edge], cameras: &CameraIndex, mut node_coord: F)
where
    F: FnMut(u64) -> (f64, f64),
{
    for edge in edges.iter_mut() {
        let (alat, alon) = node_coord(edge.from);
        let (blat, blon) = node_coord(edge.to);
        edge.exposure = edge_exposure(alat, alon, blat, blon, edge.length_m, cameras);
    }
}

/// Fraction of the A→B segment that lies inside any camera's coverage.
fn edge_exposure(
    alat: f64,
    alon: f64,
    blat: f64,
    blon: f64,
    length_m: f64,
    cameras: &CameraIndex,
) -> f64 {
    if length_m <= f64::EPSILON {
        return 0.0;
    }
    let steps = (length_m / SAMPLE_STEP_M).ceil().max(1.0) as usize;
    let mut covered = 0usize;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        // Linear interpolation in lat/lon is fine over a single OSM edge.
        let lat = alat + (blat - alat) * t;
        let lon = alon + (blon - alon) * t;
        if cameras.any_covers(lat, lon) {
            covered += 1;
        }
    }
    covered as f64 / (steps as f64 + 1.0)
}

/// Spatial index over cameras so exposure scoring doesn't go quadratic.
///
/// This is a deliberately dumb uniform grid: bucket cameras by a coarse
/// lat/lon cell, and when querying a point only test cameras in the 3×3 block
/// of cells around it. At Berlin's camera density this is plenty; swap in an
/// R-tree (`rstar`) later if a city ever gets pathological.
pub struct CameraIndex {
    cameras: Vec<Camera>,
    // Max camera range seen, used to size the query neighbourhood.
    max_range_m: f64,
}

impl CameraIndex {
    pub fn new(cameras: Vec<Camera>) -> Self {
        let max_range_m = cameras.iter().map(|c| c.range_m).fold(0.0_f64, f64::max);
        Self {
            cameras,
            max_range_m,
        }
    }

    pub fn len(&self) -> usize {
        self.cameras.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cameras.is_empty()
    }

    /// True if any camera covers this point.
    ///
    /// TODO(perf): replace this linear scan with the grid described above once
    /// the graph is wired up and we have real timing numbers. Kept linear here
    /// so the scaffold is obviously correct; `max_range_m` is already tracked
    /// so a bounding-box prefilter is a one-line change.
    pub fn any_covers(&self, lat: f64, lon: f64) -> bool {
        let _ = self.max_range_m; // used by the future grid prefilter
        self.cameras.iter().any(|c| c.covers(lat, lon))
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
        let e = edge_exposure(52.52, 13.40, 52.52, 13.41, 700.0, &idx);
        assert_eq!(e, 0.0);
    }

    #[test]
    fn edge_through_camera_scores_positive() {
        // Camera sitting right on the midpoint of a short edge.
        let idx = CameraIndex::new(vec![dome_at(52.5200, 13.4050, 30.0)]);
        let e = edge_exposure(52.5195, 13.4050, 52.5205, 13.4050, 111.0, &idx);
        assert!(e > 0.0 && e <= 1.0);
    }
}
