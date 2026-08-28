//! OSM ingest.
//!
//! Two jobs, both fed from a Geofabrik `.osm.pbf` extract of Berlin (or a live
//! Overpass dump — same tag semantics):
//!   1. pull `man_made=surveillance` nodes into [`Camera`]s
//!   2. build the walkable road graph from `highway=*` ways
//!
//! The parsing itself is left as `todo!()` — wire up the `osmpbf` crate here.
//! What this module pins down is the **tag → model mapping**, which is the part
//! that's easy to get subtly wrong and is documented in CLAUDE.md.

use crate::camera::{defaults, Camera, CameraKind};
use crate::exposure::{Edge, Node};

/// Errors from reading/decoding an OSM extract.
#[derive(Debug, thiserror::Error)]
pub enum OsmError {
    #[error("could not read OSM extract: {0}")]
    Read(String),
}

/// Map OSM surveillance tags onto a [`Camera`]. Returns `None` for nodes that
/// are tagged surveillance but aren't cameras (e.g. `surveillance:type=guard`
/// or ALPR/manned points we don't want to route around).
///
/// Relevant tags (see <https://wiki.openstreetmap.org/wiki/Key:surveillance>):
///   * `man_made=surveillance`        — the node qualifier
///   * `surveillance:type=camera`     — vs `guard` / `ALPR`
///   * `camera:type=fixed|dome|panning`
///   * `camera:direction=<deg>`       — compass bearing, cone centre
///   * `surveillance=public|outdoor|indoor|traffic`
pub fn camera_from_tags(osm_id: i64, lat: f64, lon: f64, tags: &[(String, String)]) -> Option<Camera> {
    let get = |k: &str| tags.iter().find(|(tk, _)| tk == k).map(|(_, v)| v.as_str());

    if get("man_made") != Some("surveillance") {
        return None;
    }
    // Only actual cameras. Absence of surveillance:type is treated as a camera
    // (the common mapping shorthand), but explicit non-camera types are dropped.
    match get("surveillance:type") {
        Some("camera") | None => {}
        Some(_) => return None, // guard, ALPR, etc.
    }

    let kind = match get("camera:type") {
        Some("dome") => CameraKind::Dome,
        Some("panning") => CameraKind::Panning,
        Some("fixed") => CameraKind::Fixed,
        _ => CameraKind::Unknown,
    };

    let direction_deg = get("camera:direction").and_then(|v| parse_direction(v));

    Some(Camera {
        osm_id,
        lat,
        lon,
        kind,
        direction_deg,
        half_fov_deg: defaults::half_fov_deg(kind),
        range_m: defaults::range_m(kind),
    })
}

/// OSM `camera:direction` is usually a number, but can be a compass point
/// ("N", "SW", …). Handle both.
fn parse_direction(v: &str) -> Option<f64> {
    if let Ok(deg) = v.trim().parse::<f64>() {
        return Some(((deg % 360.0) + 360.0) % 360.0);
    }
    let deg = match v.trim().to_uppercase().as_str() {
        "N" => 0.0,
        "NE" => 45.0,
        "E" => 90.0,
        "SE" => 135.0,
        "S" => 180.0,
        "SW" => 225.0,
        "W" => 270.0,
        "NW" => 315.0,
        _ => return None,
    };
    Some(deg)
}

/// Parse an entire Berlin extract into the camera set. TODO: implement with
/// `osmpbf::ElementReader`, filtering nodes by the tags above.
pub fn load_cameras(_pbf_path: &str) -> Result<Vec<Camera>, OsmError> {
    todo!("iterate osmpbf nodes, call camera_from_tags, collect Some(_)")
}

/// Build the walkable graph. TODO: implement — collect `highway=*` ways that
/// are foot-accessible, split them into per-segment [`Edge`]s with great-circle
/// lengths, and emit the [`Node`] table. Respect `access`/`foot` tags.
pub fn load_graph(_pbf_path: &str) -> Result<(Vec<Node>, Vec<Edge>), OsmError> {
    todo!("iterate osmpbf ways with highway tag, emit nodes + edges")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    #[test]
    fn parses_fixed_directional_camera() {
        let tags = vec![
            tag("man_made", "surveillance"),
            tag("surveillance:type", "camera"),
            tag("camera:type", "fixed"),
            tag("camera:direction", "90"),
        ];
        let cam = camera_from_tags(42, 52.52, 13.40, &tags).unwrap();
        assert_eq!(cam.kind, CameraKind::Fixed);
        assert_eq!(cam.direction_deg, Some(90.0));
    }

    #[test]
    fn drops_non_camera_surveillance() {
        let tags = vec![
            tag("man_made", "surveillance"),
            tag("surveillance:type", "guard"),
        ];
        assert!(camera_from_tags(1, 0.0, 0.0, &tags).is_none());
    }

    #[test]
    fn compass_point_direction() {
        let tags = vec![
            tag("man_made", "surveillance"),
            tag("camera:direction", "SW"),
        ];
        let cam = camera_from_tags(1, 0.0, 0.0, &tags).unwrap();
        assert_eq!(cam.direction_deg, Some(225.0));
    }
}
