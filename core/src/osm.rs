//! OSM ingest.
//!
//! Three jobs, all fed from a Geofabrik `.osm.pbf` extract of Berlin (or the
//! pre-filtered snapshot produced by `scripts/build_map_assets.sh` — same tag
//! semantics):
//!   1. pull `man_made=surveillance` nodes into [`Camera`]s
//!   2. build the walkable road graph from `highway=*` ways
//!   3. pull the outlines of buildings tall enough to block a camera's view
//!
//! The **tag → model mapping** is the part that's easy to get subtly wrong;
//! it is documented in CLAUDE.md and pinned down by the tests here.

use crate::camera::{defaults, haversine_m, Camera, CameraKind};
use crate::exposure::{Edge, Node};
use crate::occluders::Ring;
use crate::places::{Place, PlaceKind};
use osmpbf::{Element, ElementReader, RelMemberType};
use std::collections::{BTreeMap, BTreeSet, HashMap};

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
///   * `surveillance=public|outdoor|indoor|traffic` — `indoor` cameras are
///     dropped: they watch the inside of a building (a lobby, a museum), not
///     the street, and modelling them as a disc would hide walls' worth of
///     street behind them.
pub fn camera_from_tags(osm_id: i64, lat: f64, lon: f64, tags: &[(&str, &str)]) -> Option<Camera> {
    let get = |k: &str| tags.iter().find(|(tk, _)| *tk == k).map(|&(_, v)| v);

    if get("man_made") != Some("surveillance") {
        return None;
    }
    if get("surveillance") == Some("indoor") {
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

/// Storey height used when only `building:levels` is tagged, in metres.
const LEVEL_HEIGHT_M: f64 = 3.2;

/// Height assumed for a building with no height information at all. Berlin's
/// street-front buildings are mostly five storeys or more, so an untagged one
/// is far more likely to hide a camera's view than not.
const DEFAULT_HEIGHT_M: f64 = 9.0;

/// Height assumed for `building=` values that mean something small and low,
/// unless an explicit height says otherwise. These must not hide cameras.
const LOW_BUILDING_HEIGHT_M: f64 = 3.0;

const LOW_BUILDINGS: &[&str] = &[
    "shed",
    "garage",
    "garages",
    "carport",
    "roof",
    "hut",
    "kiosk",
    "greenhouse",
    "cabin",
];

/// Rings with more vertices than this are not buildings (they are admin
/// boundaries or lakes mistagged `building`), and are dropped rather than
/// being tested against on every sample.
const MAX_RING_POINTS: usize = 5_000;

/// Parse an OSM length value into metres: `12`, `12.5`, `12 m`, `12,5`, `40 ft`,
/// `40'`. The first of a `;`-separated list wins. Returns `None` for anything
/// non-numeric, non-positive or absurd (over 1 km).
fn parse_metres(v: &str) -> Option<f64> {
    let first = v.split(';').next()?.trim();
    let end = first
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(first.len());
    let number: f64 = first[..end].replace(',', ".").parse().ok()?;
    let unit = first[end..].trim();
    let metres = if unit.starts_with("ft") || unit.starts_with('\'') || unit.starts_with("feet") {
        number * 0.3048
    } else {
        number
    };
    (metres.is_finite() && metres > 0.0 && metres <= 1_000.0).then_some(metres)
}

/// Height of a building in metres, from the one chain of tags used everywhere:
/// `height`, then `building:height`, then `building:levels` × 3.2 m, then a
/// default by `building=` value.
fn building_height_m(tags: &[(&str, &str)]) -> f64 {
    let get = |k: &str| tags.iter().find(|(tk, _)| *tk == k).map(|&(_, v)| v);
    if let Some(h) = get("height")
        .and_then(parse_metres)
        .or_else(|| get("building:height").and_then(parse_metres))
    {
        return h;
    }
    if let Some(levels) = get("building:levels").and_then(|l| l.split(';').next()) {
        if let Ok(n) = levels.trim().parse::<f64>() {
            if n.is_finite() && n > 0.0 {
                return n * LEVEL_HEIGHT_M;
            }
        }
    }
    match get("building") {
        Some(b) if LOW_BUILDINGS.contains(&b) => LOW_BUILDING_HEIGHT_M,
        _ => DEFAULT_HEIGHT_M,
    }
}

/// Is this a building tall enough, and grounded enough, to hide things from a
/// camera? Short buildings do not. Neither do ones raised off the ground (a
/// skybridge with `min_height` above camera mounting height): the sight line
/// passes underneath.
fn blocks_cameras(tags: &[(&str, &str)]) -> bool {
    let get = |k: &str| tags.iter().find(|(tk, _)| *tk == k).map(|&(_, v)| v);
    match get("building") {
        None | Some("no") => return false,
        Some(_) => {}
    }
    let threshold = defaults::occluder_min_height_m();
    let raised = get("min_height")
        .and_then(parse_metres)
        .or_else(|| get("building:min_height").and_then(parse_metres))
        .is_some_and(|m| m >= threshold);
    !raised && building_height_m(tags) >= threshold
}

/// A candidate outline from pass 1: its node ids, and whether it is itself a
/// blocking building (as opposed to an untagged way that may turn out to be a
/// multipolygon building's outer ring).
struct WayCandidate {
    refs: Vec<i64>,
    is_building: bool,
}

/// Which candidate rings to keep, as node-id sequences. A blocking building
/// way is kept unless a blocking multipolygon relation claims it as an outer
/// ring (the relation's tags win, and it must not be counted twice); a
/// relation's outer ways are kept whether or not they were tagged themselves.
/// Outer ways that are not single closed rings (a courtyard building drawn
/// from several open segments) are skipped.
fn select_rings(
    ways: &BTreeMap<i64, WayCandidate>,
    relation_outers: &BTreeSet<i64>,
) -> Vec<Vec<i64>> {
    let own = ways
        .iter()
        .filter(|(id, w)| w.is_building && !relation_outers.contains(id))
        .map(|(_, w)| w.refs.clone());
    let from_relations = relation_outers
        .iter()
        .filter_map(|id| ways.get(id))
        .map(|w| w.refs.clone());
    own.chain(from_relations).collect()
}

/// Cameras bucketed on a coarse grid, to ask "is any camera near this box?"
/// without scanning them all for each of a few hundred thousand buildings.
struct CameraProximity {
    grid: HashMap<(i32, i32), Vec<(f64, f64)>>,
    cell_lat_deg: f64,
    cell_lon_deg: f64,
    margin_lat_deg: f64,
    margin_lon_deg: f64,
}

impl CameraProximity {
    /// Buildings matter out to the longest camera range plus a few metres.
    fn new(cameras: &[Camera]) -> Self {
        const M_PER_DEG: f64 = 111_195.0;
        let max_range = cameras.iter().map(|c| c.range_m).fold(0.0_f64, f64::max);
        let margin_m = max_range + 5.0;
        let mean_lat = cameras.iter().map(|c| c.lat).sum::<f64>() / cameras.len().max(1) as f64;
        let lon_scale = mean_lat.to_radians().cos().max(0.01);
        let cell_lat_deg = 100.0 / M_PER_DEG;
        let cell_lon_deg = cell_lat_deg / lon_scale;
        let mut grid: HashMap<(i32, i32), Vec<(f64, f64)>> = HashMap::new();
        for c in cameras {
            let key = (
                (c.lat / cell_lat_deg).floor() as i32,
                (c.lon / cell_lon_deg).floor() as i32,
            );
            grid.entry(key).or_default().push((c.lat, c.lon));
        }
        Self {
            grid,
            cell_lat_deg,
            cell_lon_deg,
            margin_lat_deg: margin_m / M_PER_DEG,
            margin_lon_deg: margin_m / (M_PER_DEG * lon_scale),
        }
    }

    /// Is any camera within the margin of this ring's bounding box?
    fn near(&self, ring: &[(f64, f64)]) -> bool {
        let (mut min_lat, mut max_lat) = (f64::MAX, f64::MIN);
        let (mut min_lon, mut max_lon) = (f64::MAX, f64::MIN);
        for &(la, lo) in ring {
            min_lat = min_lat.min(la);
            max_lat = max_lat.max(la);
            min_lon = min_lon.min(lo);
            max_lon = max_lon.max(lo);
        }
        let (min_lat, max_lat) = (min_lat - self.margin_lat_deg, max_lat + self.margin_lat_deg);
        let (min_lon, max_lon) = (min_lon - self.margin_lon_deg, max_lon + self.margin_lon_deg);
        let cell = |v: f64, size: f64| (v / size).floor() as i32;
        for i in cell(min_lat, self.cell_lat_deg)..=cell(max_lat, self.cell_lat_deg) {
            for j in cell(min_lon, self.cell_lon_deg)..=cell(max_lon, self.cell_lon_deg) {
                if let Some(cams) = self.grid.get(&(i, j)) {
                    if cams.iter().any(|&(la, lo)| {
                        (min_lat..=max_lat).contains(&la) && (min_lon..=max_lon).contains(&lo)
                    }) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Outlines of the buildings that can hide a camera's view: tagged
/// `building=*`, tall enough (see [`blocks_cameras`]), and close enough to some
/// camera to matter. A building farther than a camera's range from every
/// camera can never be between a camera and a point it covers, so dropping it
/// changes no score — it only keeps the cache and memory small.
///
/// Handles closed `building` ways and `type=multipolygon` building relations
/// whose outer rings are closed ways. Inner rings (courtyards) are ignored and
/// treated as solid: a camera inside one counts as being in its own building.
pub fn load_buildings(pbf_path: &str, cameras: &[Camera]) -> Result<Vec<Ring>, OsmError> {
    if cameras.is_empty() {
        return Ok(Vec::new());
    }

    // Pass 1: candidate outlines, and the outer ways of building relations.
    // Candidates are blocking building ways plus every way that is neither a
    // building nor a road, since a multipolygon's outer ways usually carry no
    // tags of their own. In the filtered snapshot that is a small set.
    let reader = ElementReader::from_path(pbf_path)?;
    let mut ways: BTreeMap<i64, WayCandidate> = BTreeMap::new();
    let mut relation_outers: BTreeSet<i64> = BTreeSet::new();
    reader.for_each(|element| match element {
        Element::Way(way) => {
            let tags: Vec<(&str, &str)> = way.tags().collect();
            let has = |k: &str| tags.iter().any(|(tk, _)| *tk == k);
            let is_building = blocks_cameras(&tags);
            if !is_building && (has("building") || has("highway")) {
                return;
            }
            let refs: Vec<i64> = way.refs().collect();
            if refs.len() >= 4 && refs.first() == refs.last() {
                ways.insert(way.id(), WayCandidate { refs, is_building });
            }
        }
        Element::Relation(rel) => {
            let tags: Vec<(&str, &str)> = rel.tags().collect();
            if tags.contains(&("type", "multipolygon")) && blocks_cameras(&tags) {
                for m in rel.members() {
                    if m.member_type == RelMemberType::Way && m.role().ok() == Some("outer") {
                        relation_outers.insert(m.member_id);
                    }
                }
            }
        }
        _ => {}
    })?;

    let selected = select_rings(&ways, &relation_outers);
    drop(ways);

    // Pass 2: coordinates for exactly the nodes those rings use. Ids go in a
    // sorted vector with a parallel coordinate vector — a few hundred
    // thousand buildings is millions of nodes, too many for a HashMap.
    let mut ids: Vec<i64> = selected.iter().flatten().copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let mut coords: Vec<Option<(f64, f64)>> = vec![None; ids.len()];
    let reader = ElementReader::from_path(pbf_path)?;
    reader.for_each(|element| {
        let (id, lat, lon) = match &element {
            Element::Node(n) => (n.id(), n.lat(), n.lon()),
            Element::DenseNode(n) => (n.id(), n.lat(), n.lon()),
            _ => return,
        };
        if let Ok(slot) = ids.binary_search(&id) {
            coords[slot] = Some((lat, lon));
        }
    })?;

    let near = CameraProximity::new(cameras);
    let mut rings = Vec::new();
    for refs in selected {
        // Drop the repeated closing vertex; the geometry closes implicitly.
        let open = &refs[..refs.len() - 1];
        if open.len() > MAX_RING_POINTS {
            continue;
        }
        let ring: Option<Ring> = open
            .iter()
            .map(|id| ids.binary_search(id).ok().and_then(|slot| coords[slot]))
            .collect();
        // A clipped extract can miss a node; skip the building rather than
        // invent a wall.
        if let Some(ring) = ring {
            if near.near(&ring) {
                rings.push(ring);
            }
        }
    }
    Ok(rings)
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
    fn drops_indoor_cameras_but_keeps_the_rest() {
        let with = |surveillance: &'static str| {
            camera_from_tags(
                1,
                52.52,
                13.40,
                &[
                    ("man_made", "surveillance"),
                    ("surveillance:type", "camera"),
                    ("surveillance", surveillance),
                ],
            )
        };
        assert!(with("indoor").is_none());
        for kept in ["outdoor", "public", "traffic"] {
            assert!(with(kept).is_some(), "{kept} cameras must stay");
        }
        // No `surveillance` tag at all: still a camera, as before.
        assert!(camera_from_tags(1, 0.0, 0.0, &[("man_made", "surveillance")]).is_some());
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
    fn height_chain_prefers_explicit_height_then_levels_then_default() {
        let h = |tags: &[(&str, &str)]| building_height_m(tags);
        assert_eq!(h(&[("building", "yes"), ("height", "21")]), 21.0);
        assert_eq!(h(&[("building", "yes"), ("height", "21.5 m")]), 21.5);
        assert_eq!(h(&[("building", "yes"), ("height", "12,5")]), 12.5);
        assert!((h(&[("building", "yes"), ("height", "40 ft")]) - 12.192).abs() < 1e-9);
        assert_eq!(h(&[("building", "yes"), ("building:height", "18")]), 18.0);
        // `height` beats `building:levels`.
        assert_eq!(
            h(&[
                ("building", "yes"),
                ("height", "10"),
                ("building:levels", "9")
            ]),
            10.0
        );
        assert!((h(&[("building", "yes"), ("building:levels", "5")]) - 16.0).abs() < 1e-9);
        assert_eq!(h(&[("building", "yes")]), 9.0);
        // Nonsense falls through the chain rather than poisoning it.
        assert_eq!(h(&[("building", "yes"), ("height", "tall")]), 9.0);
        assert_eq!(h(&[("building", "yes"), ("height", "-4")]), 9.0);
        assert_eq!(h(&[("building", "yes"), ("height", "99999")]), 9.0);
        // Low building classes default low, but an explicit height still wins.
        assert_eq!(h(&[("building", "garage")]), 3.0);
        assert_eq!(h(&[("building", "shed"), ("height", "8")]), 8.0);
    }

    #[test]
    fn only_tall_grounded_buildings_block_cameras() {
        assert!(blocks_cameras(&[("building", "yes")]), "untagged = 9 m");
        assert!(blocks_cameras(&[
            ("building", "apartments"),
            ("building:levels", "4")
        ]));
        // Two storeys (6.4 m) is just over the 6 m threshold; one (3.2 m) is not.
        assert!(blocks_cameras(&[
            ("building", "yes"),
            ("building:levels", "2")
        ]));
        assert!(!blocks_cameras(&[
            ("building", "yes"),
            ("building:levels", "1")
        ]));
        assert!(!blocks_cameras(&[("building", "garage")]));
        assert!(!blocks_cameras(&[("building", "yes"), ("height", "4")]));
        assert!(!blocks_cameras(&[("building", "no")]));
        assert!(!blocks_cameras(&[("highway", "footway")]));
        // A skybridge: tall, but the sight line passes beneath it.
        assert!(!blocks_cameras(&[
            ("building", "yes"),
            ("height", "12"),
            ("min_height", "8"),
        ]));
        // A raised part that still reaches the ground does block.
        assert!(blocks_cameras(&[
            ("building", "yes"),
            ("height", "12"),
            ("min_height", "2"),
        ]));
    }

    #[test]
    fn load_buildings_keeps_blockers_near_cameras_only() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mini_berlin_buildings.osm.pbf"
        );
        let cameras = load_cameras(path).unwrap();
        assert_eq!(cameras.len(), 3);
        let rings = load_buildings(path, &cameras).unwrap();
        // Kept: the 5-storey building and the multipolygon relation's outer
        // ring. Dropped: the garage (too low) and the 30 m building that is
        // kilometres from any camera.
        assert_eq!(rings.len(), 2, "{rings:?}");
        // The closing vertex is not stored twice.
        assert!(rings.iter().all(|r| r.len() == 4));
        // With no cameras nothing can matter, and nothing is read.
        assert!(load_buildings(path, &[]).unwrap().is_empty());
    }

    fn candidate(refs: &[i64], is_building: bool) -> WayCandidate {
        WayCandidate {
            refs: refs.to_vec(),
            is_building,
        }
    }

    #[test]
    fn select_rings_takes_buildings_and_relation_outers_without_duplicates() {
        let mut ways = BTreeMap::new();
        ways.insert(1, candidate(&[1, 2, 3, 1], true)); // plain building
        ways.insert(2, candidate(&[4, 5, 6, 4], false)); // untagged outer of a relation
        ways.insert(3, candidate(&[7, 8, 9, 7], true)); // building way that is also an outer
        ways.insert(4, candidate(&[10, 11, 12, 10], false)); // untagged, in no relation
        let outers: BTreeSet<i64> = [2, 3, 99].into_iter().collect(); // 99: not a candidate
        let rings = select_rings(&ways, &outers);
        assert_eq!(rings.len(), 3);
        assert!(rings.contains(&vec![1, 2, 3, 1]));
        assert!(rings.contains(&vec![4, 5, 6, 4]));
        assert!(
            rings.contains(&vec![7, 8, 9, 7]),
            "kept once, via its relation"
        );
        assert!(!rings.contains(&vec![10, 11, 12, 10]));
    }

    fn cam_at(lat: f64, lon: f64) -> Camera {
        Camera {
            osm_id: 1,
            lat,
            lon,
            kind: CameraKind::Dome,
            direction_deg: None,
            half_fov_deg: 30.0,
            range_m: 20.0,
        }
    }

    #[test]
    fn camera_proximity_keeps_only_buildings_a_camera_could_see_past() {
        let near = CameraProximity::new(&[cam_at(52.5200, 13.4050)]);
        let square = |lat: f64, lon: f64| {
            vec![
                (lat, lon),
                (lat + 0.00005, lon),
                (lat + 0.00005, lon + 0.00008),
                (lat, lon + 0.00008),
            ]
        };
        // ~11 m from the camera: kept.
        assert!(near.near(&square(52.5201, 13.4050)));
        // ~1 km away: dropped.
        assert!(!near.near(&square(52.5290, 13.4050)));
        // A long facade whose vertices are all far from the camera but whose
        // wall passes right by it must be kept (judged by extent, not vertices).
        let long = vec![
            (52.5201, 13.4000),
            (52.5201, 13.4100),
            (52.5202, 13.4100),
            (52.5202, 13.4000),
        ];
        assert!(near.near(&long));
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
