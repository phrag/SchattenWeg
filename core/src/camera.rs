//! Camera data model and field-of-view geometry.
//!
//! Cameras come from OpenStreetMap nodes tagged `man_made=surveillance`.
//! We model two coverage shapes:
//!   * a **cone** for directional/fixed cameras that carry a `camera:direction`
//!   * a **disc** for dome / panning cameras (assumed ~360°) or any camera
//!     without a known direction.
//!
//! All geometry is done on the sphere with a small-distance approximation that
//! is more than accurate enough at city scale (errors << 1 m over a few hundred
//! metres). Nothing here allocates or touches I/O, so it is cheap to call
//! millions of times during the exposure pass.

use std::f64::consts::PI;

const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// The physical mounting / movement class of a camera, derived from OSM tags
/// (`camera:type`, `surveillance:type`). This decides whether we treat its
/// coverage as a directional cone or an omnidirectional disc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CameraKind {
    /// Fixed camera pointing one way. Modelled as a cone when a direction is
    /// known, otherwise falls back to a disc.
    Fixed,
    /// Dome camera — treated as omnidirectional (disc).
    Dome,
    /// Panning / PTZ camera — can point anywhere over time, so disc.
    Panning,
    /// Unknown class — disc, the conservative (larger) coverage.
    Unknown,
}

/// A single surveillance camera with everything the exposure model needs.
///
/// `direction_deg` is a compass bearing (0 = north, 90 = east), matching OSM's
/// `camera:direction`. `range_m` is the assumed effective sight distance; it is
/// a modelling assumption, not something OSM usually provides, so it is set per
/// camera from a default table (see `defaults` module) and can be tuned later.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Camera {
    /// OSM node id, kept for provenance and de-duplication.
    pub osm_id: i64,
    pub lat: f64,
    pub lon: f64,
    pub kind: CameraKind,
    /// Compass bearing the camera faces, if known. `None` => omnidirectional.
    pub direction_deg: Option<f64>,
    /// Half-angle of the cone in degrees (e.g. 30 => a 60° field of view).
    /// Ignored for discs.
    pub half_fov_deg: f64,
    /// Assumed effective range in metres.
    pub range_m: f64,
}

impl Camera {
    /// True if `(lat, lon)` falls inside this camera's modelled coverage.
    ///
    /// This is the hot inner check of the whole exposure pass. Order matters:
    /// the cheap radius test runs first and rejects the vast majority of
    /// points before we ever compute a bearing.
    pub fn covers(&self, lat: f64, lon: f64) -> bool {
        let d = haversine_m(self.lat, self.lon, lat, lon);
        if d > self.range_m {
            return false;
        }
        match (self.kind, self.direction_deg) {
            // Directional cone: point must also lie within the angular sweep.
            (CameraKind::Fixed, Some(dir)) => {
                let bearing = bearing_deg(self.lat, self.lon, lat, lon);
                angular_within(bearing, dir, self.half_fov_deg)
            }
            // Everything else is a disc; the radius test above is sufficient.
            _ => true,
        }
    }
}

/// Great-circle distance in metres (haversine).
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().asin()
}

/// Initial compass bearing in degrees from point 1 to point 2 (0 = north).
pub fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    let brng = y.atan2(x); // radians, -pi..pi
    (brng * 180.0 / PI + 360.0) % 360.0
}

/// True if `bearing` lies within `half_fov` degrees of `center`, wrapping
/// correctly across the 0/360 boundary.
fn angular_within(bearing: f64, center: f64, half_fov: f64) -> bool {
    let mut diff = (bearing - center).abs() % 360.0;
    if diff > 180.0 {
        diff = 360.0 - diff;
    }
    diff <= half_fov
}

/// Default modelling parameters keyed off the OSM tag classes. These are
/// deliberately conservative starting points — tune against ground truth.
pub mod defaults {
    use super::CameraKind;

    /// Effective range in metres by camera class.
    pub fn range_m(kind: CameraKind) -> f64 {
        match kind {
            CameraKind::Fixed => 25.0,
            CameraKind::Dome => 20.0,
            CameraKind::Panning => 30.0,
            CameraKind::Unknown => 20.0,
        }
    }

    /// Half field-of-view in degrees for directional cameras.
    pub fn half_fov_deg(_kind: CameraKind) -> f64 {
        30.0 // i.e. a 60° cone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_facing(dir: f64) -> Camera {
        Camera {
            osm_id: 1,
            lat: 52.5200,
            lon: 13.4050,
            kind: CameraKind::Fixed,
            direction_deg: Some(dir),
            half_fov_deg: 30.0,
            range_m: 50.0,
        }
    }

    #[test]
    fn disc_covers_within_radius() {
        let cam = Camera {
            kind: CameraKind::Dome,
            direction_deg: None,
            ..fixed_facing(0.0)
        };
        // ~10 m north — inside 20 m default disc.
        assert!(cam.covers(52.5200 + 0.00009, 13.4050));
    }

    #[test]
    fn cone_rejects_behind_camera() {
        // Camera faces north; a point due south must be excluded even if close.
        let cam = fixed_facing(0.0);
        assert!(!cam.covers(52.5200 - 0.0002, 13.4050));
    }

    #[test]
    fn cone_accepts_in_front() {
        let cam = fixed_facing(0.0);
        assert!(cam.covers(52.5200 + 0.0002, 13.4050));
    }
}
