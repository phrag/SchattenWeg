//! Buildings as line-of-sight blockers for camera coverage.
//!
//! A camera cannot see through a wall. Without this, [`Camera::covers`] is a
//! pure range-and-bearing test, so a camera facing a building "watches" the
//! street on its far side and both the exposure score and the map overstate
//! what it can see. An [`OccluderIndex`] holds the footprints of buildings tall
//! enough to hide things (see `defaults::occluder_min_height_m`; the height
//! filter is applied at ingest in `osm.rs`, so everything in here blocks) and
//! answers two questions from the same geometry:
//!
//!   * [`OccluderIndex::blocks`] — does a wall lie between a camera and a
//!     point? Used by the exposure pass.
//!   * [`OccluderIndex::coverage_ring`] — the camera's coverage outline with
//!     the blocked parts cut away. Used by the map, so the picture is the
//!     model rather than a second copy of it.
//!
//! Everything errs towards *more* exposure, the safe direction for a
//! surveillance-avoidance tool:
//!   * the camera's **own building** is ignored (cameras sit on facades and
//!     roofs, so the wall behind them must not hide the street in front);
//!   * a building that **contains the target point** is ignored, so walkable
//!     passages and `highway=corridor` edges through a building stay watched;
//!   * a sight line that merely grazes a vertex is not blocked;
//!   * only outer rings are used, so courtyards are solid. A camera inside
//!     one is "in its own building" and sees out regardless.
//!
//! The index is a uniform grid over ring bounding boxes, like `CameraIndex`.
//! Rings are registered by bounding box rather than centroid so a long
//! building is found from anywhere along it.

use crate::camera::{Camera, CameraKind, EARTH_RADIUS_M};
use std::collections::HashMap;
use std::f64::consts::PI;

/// A closed building outline as `(lat, lon)` vertices. The first vertex may or
/// may not be repeated at the end; both are accepted.
pub type Ring = Vec<(f64, f64)>;

/// Metres per degree of latitude on the same sphere `haversine_m` uses, so the
/// ranges in here agree with `Camera::covers` to well under a metre.
const M_PER_DEG: f64 = EARTH_RADIUS_M * PI / 180.0;

/// Grid cell edge in metres. Sight lines are at most a camera's range (tens of
/// metres) long, so a query touches a handful of cells.
const CELL_M: f64 = 50.0;

/// A camera this close to a building's outline (or inside it) is treated as
/// mounted on it.
const OWN_WALL_M: f64 = 3.0;

/// Rays used to trace a cone / a full disc when drawing coverage.
const CONE_RAYS: usize = 48;
const DISC_RAYS: usize = 72;

/// A ring spanning more grid cells than this is dropped as corrupt data (it
/// would be a building several kilometres across).
const MAX_CELLS_PER_RING: i64 = 10_000;

struct Entry {
    ring: Ring,
    min_lat: f64,
    max_lat: f64,
    min_lon: f64,
    max_lon: f64,
}

/// Spatial index over blocking building outlines.
pub struct OccluderIndex {
    rings: Vec<Entry>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    cell_lat_deg: f64,
    cell_lon_deg: f64,
}

/// Flat east/north metre frame centred on a camera.
struct Local {
    lat0: f64,
    lon0: f64,
    kx: f64,
}

impl Local {
    fn new(lat0: f64, lon0: f64) -> Self {
        Self {
            lat0,
            lon0,
            kx: M_PER_DEG * lat0.to_radians().cos(),
        }
    }

    fn xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        ((lon - self.lon0) * self.kx, (lat - self.lat0) * M_PER_DEG)
    }
}

impl OccluderIndex {
    /// An index with no buildings: nothing blocks, rings are plain wedges/discs.
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    pub fn new(rings: Vec<Ring>) -> Self {
        let rings: Vec<Ring> = rings
            .into_iter()
            .filter(|r| r.len() >= 3 && r.iter().all(|&(a, b)| a.is_finite() && b.is_finite()))
            .collect();
        let mean_lat = if rings.is_empty() {
            0.0
        } else {
            rings.iter().map(|r| r[0].0).sum::<f64>() / rings.len() as f64
        };
        let cell_lat_deg = CELL_M / M_PER_DEG;
        let cell_lon_deg = CELL_M / (M_PER_DEG * mean_lat.to_radians().cos().max(0.01));

        let mut entries = Vec::with_capacity(rings.len());
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for ring in rings {
            let (mut min_lat, mut max_lat) = (f64::MAX, f64::MIN);
            let (mut min_lon, mut max_lon) = (f64::MAX, f64::MIN);
            for &(la, lo) in &ring {
                min_lat = min_lat.min(la);
                max_lat = max_lat.max(la);
                min_lon = min_lon.min(lo);
                max_lon = max_lon.max(lo);
            }
            let (i0, i1) = (cell(min_lat, cell_lat_deg), cell(max_lat, cell_lat_deg));
            let (j0, j1) = (cell(min_lon, cell_lon_deg), cell(max_lon, cell_lon_deg));
            if (i64::from(i1) - i64::from(i0) + 1) * (i64::from(j1) - i64::from(j0) + 1)
                > MAX_CELLS_PER_RING
            {
                continue;
            }
            let id = entries.len() as u32;
            for i in i0..=i1 {
                for j in j0..=j1 {
                    grid.entry((i, j)).or_default().push(id);
                }
            }
            entries.push(Entry {
                ring,
                min_lat,
                max_lat,
                min_lon,
                max_lon,
            });
        }
        Self {
            rings: entries,
            grid,
            cell_lat_deg,
            cell_lon_deg,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rings.is_empty()
    }

    /// Rings overlapping the bounding box of the segment `a`–`b` (each once).
    fn candidates(&self, a: (f64, f64), b: (f64, f64)) -> Vec<u32> {
        let (min_lat, max_lat) = (a.0.min(b.0), a.0.max(b.0));
        let (min_lon, max_lon) = (a.1.min(b.1), a.1.max(b.1));
        let mut out = Vec::new();
        for i in cell(min_lat, self.cell_lat_deg)..=cell(max_lat, self.cell_lat_deg) {
            for j in cell(min_lon, self.cell_lon_deg)..=cell(max_lon, self.cell_lon_deg) {
                if let Some(ids) = self.grid.get(&(i, j)) {
                    out.extend_from_slice(ids);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out.retain(|&id| {
            let e = &self.rings[id as usize];
            e.min_lat <= max_lat
                && e.max_lat >= min_lat
                && e.min_lon <= max_lon
                && e.max_lon >= min_lon
        });
        out
    }

    /// True if a wall stands between the camera at `cam` and the point `p`.
    pub fn blocks(&self, cam: (f64, f64), p: (f64, f64)) -> bool {
        if self.rings.is_empty() {
            return false;
        }
        let local = Local::new(cam.0, cam.1);
        let target = local.xy(p.0, p.1);
        for id in self.candidates(cam, p) {
            let ring = project(&self.rings[id as usize].ring, &local);
            // Cheapest test first; the exemptions below only matter for the
            // rare building the sight line actually crosses.
            if !crosses(&ring, (0.0, 0.0), target) {
                continue;
            }
            if is_own_building(&ring) || point_in_ring(&ring, target) {
                continue;
            }
            return true;
        }
        false
    }

    /// How far the camera sees along `bearing_deg` (0 = north), up to
    /// `max_range_m`: the distance to the first wall, or `max_range_m`.
    pub fn visible_range(&self, cam: (f64, f64), bearing_deg: f64, max_range_m: f64) -> f64 {
        if self.rings.is_empty() {
            return max_range_m;
        }
        let local = Local::new(cam.0, cam.1);
        let b = bearing_deg.to_radians();
        let dir = (b.sin(), b.cos());
        let end = (
            cam.0 + max_range_m * dir.1 / M_PER_DEG,
            cam.1 + max_range_m * dir.0 / local.kx,
        );
        let mut best = max_range_m;
        for id in self.candidates(cam, end) {
            let ring = project(&self.rings[id as usize].ring, &local);
            if is_own_building(&ring) {
                continue;
            }
            if let Some(t) = first_ray_hit(&ring, dir, best) {
                best = best.min(t);
            }
        }
        best
    }

    /// The camera's coverage outline with blocked directions cut short: a
    /// closed `(lat, lon)` ring — a wedge for a fixed camera with a bearing, a
    /// disc for everything else, mirroring [`Camera::covers`]. It is a polygon
    /// traced with a fixed number of rays, so it can differ from `covers` +
    /// [`OccluderIndex::blocks`] by a metre or two at corners.
    pub fn coverage_ring(&self, cam: &Camera) -> Ring {
        let origin = (cam.lat, cam.lon);
        let kx = M_PER_DEG * cam.lat.to_radians().cos().max(0.01);
        let point_at = |bearing: f64| {
            let d = self.visible_range(origin, bearing, cam.range_m);
            let b = bearing.to_radians();
            (
                cam.lat + d * b.cos() / M_PER_DEG,
                cam.lon + d * b.sin() / kx,
            )
        };
        let mut ring = Vec::new();
        match (cam.kind, cam.direction_deg) {
            (CameraKind::Fixed, Some(dir)) => {
                ring.push(origin);
                for i in 0..=CONE_RAYS {
                    let t = i as f64 / CONE_RAYS as f64;
                    ring.push(point_at(
                        dir - cam.half_fov_deg + 2.0 * cam.half_fov_deg * t,
                    ));
                }
                ring.push(origin);
            }
            _ => {
                for i in 0..DISC_RAYS {
                    ring.push(point_at(360.0 * i as f64 / DISC_RAYS as f64));
                }
                ring.push(ring[0]);
            }
        }
        ring
    }
}

fn cell(v: f64, size: f64) -> i32 {
    (v / size).floor() as i32
}

fn project(ring: &[(f64, f64)], local: &Local) -> Vec<(f64, f64)> {
    ring.iter().map(|&(la, lo)| local.xy(la, lo)).collect()
}

fn cross(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

fn sub(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 - b.0, a.1 - b.1)
}

/// Does the open segment `o`–`p` properly cross any edge of the ring? Touching
/// or running along an edge, and passing through a vertex, do not count: a
/// grazing sight line is not blocked. "Through a vertex" is judged with a
/// small tolerance (the cross products are in m², so this is nanometres) so
/// float noise cannot turn a graze into a block.
fn crosses(ring: &[(f64, f64)], o: (f64, f64), p: (f64, f64)) -> bool {
    const EPS: f64 = 1e-9;
    let opposite = |x: f64, y: f64| (x < -EPS && y > EPS) || (x > EPS && y < -EPS);
    let n = ring.len();
    let op = sub(p, o);
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let ab = sub(b, a);
        if opposite(cross(ab, sub(o, a)), cross(ab, sub(p, a)))
            && opposite(cross(op, sub(a, o)), cross(op, sub(b, o)))
        {
            return true;
        }
    }
    false
}

/// Distance along a ray from the origin to the nearest edge it crosses, if that
/// is within `max_t`.
fn first_ray_hit(ring: &[(f64, f64)], dir: (f64, f64), max_t: f64) -> Option<f64> {
    let n = ring.len();
    let mut best: Option<f64> = None;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let e = sub(b, a);
        let denom = cross(dir, e);
        if denom.abs() < 1e-12 {
            continue; // parallel
        }
        let t = cross(a, e) / denom;
        let u = cross(a, dir) / denom;
        if t > 1e-6 && t <= max_t && (0.0..=1.0).contains(&u) && best.is_none_or(|bt| t < bt) {
            best = Some(t);
        }
    }
    best
}

/// Even-odd point-in-polygon test.
fn point_in_ring(ring: &[(f64, f64)], p: (f64, f64)) -> bool {
    let n = ring.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Is the frame's origin (the camera) inside this ring or hugging its outline?
fn is_own_building(ring: &[(f64, f64)]) -> bool {
    if point_in_ring(ring, (0.0, 0.0)) {
        return true;
    }
    let n = ring.len();
    (0..n).any(|i| dist_to_segment((0.0, 0.0), ring[i], ring[(i + 1) % n]) < OWN_WALL_M)
}

fn dist_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let ab = sub(b, a);
    let len2 = ab.0 * ab.0 + ab.1 * ab.1;
    let t = if len2 <= f64::EPSILON {
        0.0
    } else {
        ((p.0 - a.0) * ab.0 + (p.1 - a.1) * ab.1) / len2
    }
    .clamp(0.0, 1.0);
    let (cx, cy) = (a.0 + t * ab.0, a.1 + t * ab.1);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::haversine_m;

    const LAT: f64 = 52.5200;
    const LON: f64 = 13.4050;
    const M_LAT: f64 = 1.0 / M_PER_DEG;

    fn m_lon() -> f64 {
        M_LAT / LAT.to_radians().cos()
    }

    /// A rectangle `south..north` metres and `west..east` metres from (LAT, LON).
    fn rect(south: f64, north: f64, west: f64, east: f64) -> Ring {
        vec![
            (LAT + south * M_LAT, LON + west * m_lon()),
            (LAT + north * M_LAT, LON + west * m_lon()),
            (LAT + north * M_LAT, LON + east * m_lon()),
            (LAT + south * M_LAT, LON + east * m_lon()),
        ]
    }

    fn at(north_m: f64, east_m: f64) -> (f64, f64) {
        (LAT + north_m * M_LAT, LON + east_m * m_lon())
    }

    /// A 10 m-wide wall of a building 10..20 m due north of the camera.
    fn wall_north() -> OccluderIndex {
        OccluderIndex::new(vec![rect(10.0, 20.0, -5.0, 5.0)])
    }

    fn dome(range: f64) -> Camera {
        Camera {
            osm_id: 1,
            lat: LAT,
            lon: LON,
            kind: CameraKind::Dome,
            direction_deg: None,
            half_fov_deg: 30.0,
            range_m: range,
        }
    }

    #[test]
    fn a_wall_between_camera_and_point_blocks() {
        let idx = wall_north();
        assert!(idx.blocks((LAT, LON), at(25.0, 0.0)));
    }

    #[test]
    fn a_point_beside_the_building_is_not_blocked() {
        let idx = wall_north();
        assert!(!idx.blocks((LAT, LON), at(15.0, 12.0)));
        assert!(!idx.blocks((LAT, LON), at(-20.0, 0.0)));
    }

    #[test]
    fn a_point_before_the_wall_is_not_blocked() {
        assert!(!wall_north().blocks((LAT, LON), at(8.0, 0.0)));
    }

    #[test]
    fn a_camera_on_its_own_building_sees_out() {
        // Camera on the south facade, 1 m from the wall: that building is its
        // own, so the street to the north-side is not hidden by it.
        let idx = wall_north();
        let cam = at(9.0, 0.0);
        assert!(!idx.blocks(cam, at(30.0, 0.0)));
        // And inside the footprint (a rooftop camera) likewise.
        assert!(!idx.blocks(at(15.0, 0.0), at(30.0, 0.0)));
    }

    #[test]
    fn a_point_inside_a_building_stays_visible() {
        // A passage / corridor edge through the building is still watched.
        let idx = wall_north();
        assert!(!idx.blocks((LAT, LON), at(15.0, 0.0)));
    }

    #[test]
    fn grazing_a_vertex_does_not_block() {
        // Sight line exactly through the building's SW corner, then past it.
        let idx = wall_north();
        let target = at(20.0, -10.0); // line through (10, -5)
        assert!(!idx.blocks((LAT, LON), target));
    }

    #[test]
    fn empty_index_blocks_nothing() {
        assert!(!OccluderIndex::empty().blocks((LAT, LON), at(25.0, 0.0)));
    }

    #[test]
    fn degenerate_rings_are_ignored() {
        let idx = OccluderIndex::new(vec![vec![(LAT, LON)], vec![(f64::NAN, LON); 4]]);
        assert!(idx.is_empty());
    }

    #[test]
    fn a_long_building_is_found_from_anywhere_along_it() {
        // 400 m facade, centroid far from the sight line.
        let idx = OccluderIndex::new(vec![rect(10.0, 14.0, -200.0, 200.0)]);
        assert!(idx.blocks((LAT, LON), at(20.0, 0.0)));
        let cam = at(0.0, -190.0);
        assert!(idx.blocks(cam, (cam.0 + 20.0 * M_LAT, cam.1)));
    }

    #[test]
    fn visible_range_stops_at_the_wall() {
        let idx = wall_north();
        let r = idx.visible_range((LAT, LON), 0.0, 30.0);
        assert!((r - 10.0).abs() < 0.2, "{r}");
        assert_eq!(idx.visible_range((LAT, LON), 180.0, 30.0), 30.0);
        assert_eq!(idx.visible_range((LAT, LON), 90.0, 30.0), 30.0);
    }

    #[test]
    fn coverage_ring_is_cut_by_the_wall_and_closed() {
        let idx = wall_north();
        let ring = idx.coverage_ring(&dome(30.0));
        assert_eq!(ring.first(), ring.last());
        let north = ring
            .iter()
            .map(|&(la, lo)| haversine_m(LAT, LON, la, lo))
            .fold(0.0_f64, f64::max);
        assert!((north - 30.0).abs() < 0.5, "south side still reaches range");
        let nearest_north = ring
            .iter()
            .filter(|&&(la, _)| la > LAT)
            .map(|&(la, lo)| haversine_m(LAT, LON, la, lo))
            .fold(f64::MAX, f64::min);
        assert!(nearest_north <= 12.0, "north side stops at the wall");
    }

    #[test]
    fn unobstructed_coverage_ring_is_a_plain_disc_or_wedge() {
        let none = OccluderIndex::empty();
        let disc = none.coverage_ring(&dome(20.0));
        for &(la, lo) in &disc[..disc.len() - 1] {
            assert!((haversine_m(LAT, LON, la, lo) - 20.0).abs() < 0.3);
        }
        let fixed = Camera {
            kind: CameraKind::Fixed,
            direction_deg: Some(90.0),
            ..dome(25.0)
        };
        let wedge = none.coverage_ring(&fixed);
        assert_eq!(wedge.first(), Some(&(LAT, LON)));
        assert_eq!(wedge.last(), Some(&(LAT, LON)));
        // Every arc point is due-east-ish: no vertex west of the camera.
        assert!(wedge.iter().all(|&(_, lo)| lo >= LON));
    }

    #[test]
    fn coverage_ring_agrees_with_covers_and_blocks() {
        // The drawn outline and the scoring rule must tell the same story.
        let idx = OccluderIndex::new(vec![
            rect(8.0, 14.0, -6.0, 3.0),
            rect(-14.0, -9.0, 4.0, 15.0),
        ]);
        let cam = dome(30.0);
        let ring = idx.coverage_ring(&cam);
        let mut mismatches = 0;
        let mut total = 0;
        for i in -35..=35 {
            for j in -35..=35 {
                let p = at(f64::from(i), f64::from(j));
                if !cam.covers(p.0, p.1) {
                    continue;
                }
                // Skip points inside a building: `blocks` deliberately lets the
                // camera see into passages, the outline stops at the wall.
                let inside_a_building = (8.0..=14.0).contains(&f64::from(i))
                    && (-6.0..=3.0).contains(&f64::from(j))
                    || (-14.0..=-9.0).contains(&f64::from(i))
                        && (4.0..=15.0).contains(&f64::from(j));
                if inside_a_building {
                    continue;
                }
                total += 1;
                let visible = !idx.blocks((LAT, LON), p);
                let in_ring = point_in_ring(
                    &ring.iter().map(|&(la, lo)| (lo, la)).collect::<Vec<_>>(),
                    (p.1, p.0),
                );
                if visible != in_ring {
                    mismatches += 1;
                }
            }
        }
        assert!(total > 500);
        // Ray discretisation only disagrees in a thin sliver along each
        // shadow boundary.
        assert!(mismatches * 10 < total, "{mismatches}/{total} disagree");
    }

    #[test]
    fn grid_matches_brute_force() {
        // A scatter of blocks; the grid query must equal a linear scan.
        let mut rings = Vec::new();
        for k in 0..30u32 {
            let n = f64::from(k % 6) * 17.0 - 40.0;
            let e = f64::from(k / 6) * 19.0 - 40.0;
            rings.push(rect(n, n + 8.0, e, e + 11.0));
        }
        let idx = OccluderIndex::new(rings.clone());
        let local = Local::new(LAT, LON);
        for i in -45..=45 {
            for j in -45..=45 {
                let p = at(f64::from(i) * 1.1, f64::from(j) * 0.9);
                let target = local.xy(p.0, p.1);
                let brute = rings.iter().any(|r| {
                    let ring = project(r, &local);
                    crosses(&ring, (0.0, 0.0), target)
                        && !is_own_building(&ring)
                        && !point_in_ring(&ring, target)
                });
                assert_eq!(idx.blocks((LAT, LON), p), brute, "at {i},{j}");
            }
        }
    }
}
