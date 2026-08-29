//! OSM ingest.
//!
//! Two jobs, both fed from a Geofabrik `.osm.pbf` extract of Berlin (or the
//! pre-filtered snapshot produced by `scripts/build_map_assets.sh` — same tag
//! semantics):
//!   1. pull `man_made=surveillance` nodes into [`Camera`]s
//!   2. build the walkable road graph from `highway=*` ways
//!
//! The **tag → model mapping** is the part that's easy to get subtly wrong;
//! it is documented in CLAUDE.md and pinned down by the tests here.

use crate::camera::{defaults, haversine_m, Camera, CameraKind};
use crate::exposure::{Edge, Node};
use crate::places::{Place, PlaceKind};
use osmpbf::{Element, ElementReader};
use std::collections::HashMap;

/// Errors from reading/decoding an OSM extract.
#[derive(Debug, thiserror::Error)]
pub enum OsmError {
    #[error("could not read OSM extract: {0}")]
    Read(String),
}

impl From<osmpbf::Error> for OsmError {
    fn from(e: osmpbf::Error) -> Self {
        OsmError::Read(e.to_string())
    }
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
pub fn camera_from_tags(osm_id: i64, lat: f64, lon: f64, tags: &[(&str, &str)]) -> Option<Camera> {
    let get = |k: &str| tags.iter().find(|(tk, _)| *tk == k).map(|&(_, v)| v);

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

    let direction_deg = get("camera:direction").and_then(parse_direction);

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

/// Parse an entire extract into the camera set.
pub fn load_cameras(pbf_path: &str) -> Result<Vec<Camera>, OsmError> {
    let reader = ElementReader::from_path(pbf_path)?;
    let mut cameras = Vec::new();
    reader.for_each(|element| {
        let (id, lat, lon, tags): (i64, f64, f64, Vec<(&str, &str)>) = match &element {
            Element::Node(n) => (n.id(), n.lat(), n.lon(), n.tags().collect()),
            Element::DenseNode(n) => (n.id(), n.lat(), n.lon(), n.tags().collect()),
            _ => return,
        };
        if let Some(cam) = camera_from_tags(id, lat, lon, &tags) {
            cameras.push(cam);
        }
    })?;
    Ok(cameras)
}

/// Is this `highway=*` way walkable on foot?
///
/// Whitelist of walkable classes plus the usual German-city access rules:
/// explicit `foot=yes|designated|permissive` overrides restrictive `access`;
/// `foot=no|private|use_sidepath` always excludes; `access=no|private` excludes
/// unless foot explicitly allows. Cycleways only count when foot is allowed.
fn foot_accessible(tags: &[(&str, &str)]) -> bool {
    let get = |k: &str| tags.iter().find(|(tk, _)| *tk == k).map(|&(_, v)| v);

    let Some(highway) = get("highway") else {
        return false;
    };

    const WALKABLE: &[&str] = &[
        "footway",
        "path",
        "pedestrian",
        "steps",
        "corridor",
        "living_street",
        "residential",
        "service",
        "track",
        "bridleway",
        "unclassified",
        "tertiary",
        "tertiary_link",
        "secondary",
        "secondary_link",
        "primary",
        "primary_link",
        "road",
    ];

    let foot = get("foot");
    let foot_allows = matches!(foot, Some("yes") | Some("designated") | Some("permissive"));

    // Cycleways are foot-forbidden by default in Germany; include only when
    // explicitly opened to pedestrians.
    let class_ok = WALKABLE.contains(&highway) || (highway == "cycleway" && foot_allows);
    if !class_ok {
        return false;
    }
    if matches!(foot, Some("no") | Some("private") | Some("use_sidepath")) {
        return false;
    }
    if matches!(get("access"), Some("no") | Some("private")) && !foot_allows {
        return false;
    }
    true
}

/// The routable network plus the names a user can search for.
pub struct Network {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub places: Vec<Place>,
}

/// Build the walkable graph and the searchable place list in the same two
/// passes: ways first (to learn which node ids we need, and to note the named
/// streets), then nodes (to resolve coordinates, and to pick up places and
/// stations). Adding a separate pass for names would have cost another full
/// scan of the extract on every cold start, which is already the slow part.
pub fn load_network(pbf_path: &str) -> Result<Network, OsmError> {
    // Pass 1: node-id sequences of every walkable way.
    let reader = ElementReader::from_path(pbf_path)?;
    let mut way_node_seqs: Vec<Vec<i64>> = Vec::new();
    // Street name -> a node on it, resolved to a coordinate in pass 2. Many
    // ways share a name (a street is split at every junction), so the first
    // one wins and the rest are ignored.
    let mut street_anchor: HashMap<String, i64> = HashMap::new();
    reader.for_each(|element| {
        if let Element::Way(way) = element {
            let tags: Vec<(&str, &str)> = way.tags().collect();
            let get = |k: &str| tags.iter().find(|(tk, _)| *tk == k).map(|&(_, v)| v);
            if get("highway").is_some() {
                if let Some(name) = get("name") {
                    if let Some(first) = way.refs().next() {
                        street_anchor.entry(name.to_string()).or_insert(first);
                    }
                }
            }
            if foot_accessible(&tags) {
                way_node_seqs.push(way.refs().collect());
            }
        }
    })?;

    let mut needed: HashMap<i64, Option<(f64, f64)>> = HashMap::new();
    for seq in &way_node_seqs {
        for &id in seq {
            needed.insert(id, None);
        }
    }
    // Street anchors must be resolved too, even when the way itself is not
    // walkable (a named road we route around is still worth searching for).
    for &id in street_anchor.values() {
        needed.entry(id).or_insert(None);
    }
    let mut places: Vec<Place> = Vec::new();

    // Pass 2: coordinates for exactly those nodes.
    let reader = ElementReader::from_path(pbf_path)?;
    reader.for_each(|element| {
        let (id, lat, lon, tags): (i64, f64, f64, Vec<(&str, &str)>) = match &element {
            Element::Node(n) => (n.id(), n.lat(), n.lon(), n.tags().collect()),
            Element::DenseNode(n) => (n.id(), n.lat(), n.lon(), n.tags().collect()),
            _ => return,
        };
        if let Some(slot) = needed.get_mut(&id) {
            *slot = Some((lat, lon));
        }
        if let Some(place) = place_from_tags(lat, lon, &tags) {
            places.push(place);
        }
    })?;

    for (name, anchor) in street_anchor {
        if let Some(Some((lat, lon))) = needed.get(&anchor) {
            places.push(Place {
                name,
                kind: PlaceKind::Street,
                lat: *lat,
                lon: *lon,
            });
        }
    }

    let nodes: Vec<Node> = needed
        .iter()
        .filter_map(|(&id, coord)| {
            coord.map(|(lat, lon)| Node {
                id: id as u64,
                lat,
                lon,
            })
        })
        .collect();

    let mut edges = Vec::new();
    for seq in &way_node_seqs {
        for pair in seq.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            // Ways can reference nodes missing from a clipped extract; skip
            // those segments rather than inventing zero-length geometry.
            let (Some(Some((alat, alon))), Some(Some((blat, blon)))) =
                (needed.get(&a), needed.get(&b))
            else {
                continue;
            };
            let length_m = haversine_m(*alat, *alon, *blat, *blon);
            // Walking is direction-agnostic: emit both directions.
            edges.push(Edge {
                from: a as u64,
                to: b as u64,
                length_m,
                exposure: 0.0,
            });
            edges.push(Edge {
                from: b as u64,
                to: a as u64,
                length_m,
                exposure: 0.0,
            });
        }
    }

    Ok(Network {
        nodes,
        edges,
        places,
    })
}

/// Localities and transit stops worth searching for. Deliberately narrow:
/// every shop and bench in OSM would bury the names people actually navigate
/// by.
fn place_from_tags(lat: f64, lon: f64, tags: &[(&str, &str)]) -> Option<Place> {
    let get = |k: &str| tags.iter().find(|(tk, _)| *tk == k).map(|&(_, v)| v);
    let name = get("name")?;

    const LOCALITIES: &[&str] = &[
        "city",
        "borough",
        "suburb",
        "quarter",
        "neighbourhood",
        "town",
        "village",
    ];
    let kind = match get("place") {
        Some(p) if LOCALITIES.contains(&p) => PlaceKind::Locality,
        _ => {
            let is_station = matches!(get("railway"), Some("station") | Some("halt"))
                || get("public_transport") == Some("station");
            if is_station {
                PlaceKind::Station
            } else {
                return None;
            }
        }
    };

    Some(Place {
        name: name.to_string(),
        kind,
        lat,
        lon,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixed_directional_camera() {
        let tags = [
            ("man_made", "surveillance"),
            ("surveillance:type", "camera"),
            ("camera:type", "fixed"),
            ("camera:direction", "90"),
        ];
        let cam = camera_from_tags(42, 52.52, 13.40, &tags).unwrap();
        assert_eq!(cam.kind, CameraKind::Fixed);
        assert_eq!(cam.direction_deg, Some(90.0));
    }

    #[test]
    fn drops_non_camera_surveillance() {
        let tags = [("man_made", "surveillance"), ("surveillance:type", "guard")];
        assert!(camera_from_tags(1, 0.0, 0.0, &tags).is_none());
    }

    #[test]
    fn compass_point_direction() {
        let tags = [("man_made", "surveillance"), ("camera:direction", "SW")];
        let cam = camera_from_tags(1, 0.0, 0.0, &tags).unwrap();
        assert_eq!(cam.direction_deg, Some(225.0));
    }

    #[test]
    fn footways_walkable_motorways_not() {
        assert!(foot_accessible(&[("highway", "footway")]));
        assert!(foot_accessible(&[("highway", "residential")]));
        assert!(!foot_accessible(&[("highway", "motorway")]));
        assert!(!foot_accessible(&[("highway", "trunk")]));
    }

    #[test]
    fn foot_and_access_tags_respected() {
        assert!(!foot_accessible(&[("highway", "path"), ("foot", "no")]));
        assert!(!foot_accessible(&[
            ("highway", "service"),
            ("access", "private")
        ]));
        // Explicit foot permission overrides a restrictive access tag.
        assert!(foot_accessible(&[
            ("highway", "service"),
            ("access", "private"),
            ("foot", "yes"),
        ]));
        // Cycleways only when opened to pedestrians.
        assert!(!foot_accessible(&[("highway", "cycleway")]));
        assert!(foot_accessible(&[("highway", "cycleway"), ("foot", "yes")]));
    }
}
