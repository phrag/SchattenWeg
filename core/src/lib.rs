//! schattenweg-core
//! ================
//! Rust core for the Schattenweg app: OSM ingest, surveillance-exposure
//! scoring, and camera-aware routing. Everything runs on-device — no network,
//! no server. The UI (Kotlin/Compose) talks to exactly the surface defined
//! here through UniFFI.
//!
//! Design note: keep this boundary *small and value-typed*. Heavy state (the
//! graph, the camera index) lives behind a single `Router` object so the FFI
//! never marshals the whole graph across the language boundary.

mod cache;
mod camera;
mod exposure;
mod osm;
mod places;
mod routing;

pub use camera::{Camera, CameraKind};
pub use exposure::{CameraIndex, Edge, Node};
pub use places::{Place, PlaceIndex, PlaceKind};

use routing::Graph;
use std::collections::HashMap;
use std::sync::Arc;

uniffi::setup_scaffolding!();

/// Errors surfaced to the app layer.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum RouteError {
    // Deliberately NOT named `message`: UniFFI maps an error field onto a
    // Kotlin class that already inherits `message` from Throwable, and emits
    // both `val message` and `override val message` in the same class, which
    // does not compile. Any field name but `message` avoids the collision.
    #[error("failed to load map data: {reason}")]
    LoadFailed { reason: String },
    #[error("no graph node near the given start/end point")]
    NoNearbyNode,
    #[error("no route exists between the given points")]
    Unreachable,
}

/// A geographic point handed in from the UI (a map tap or GPS fix).
#[derive(Debug, Clone, Copy, uniffi::Record)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

/// A planned route returned to the UI. `polyline` is the ordered list of
/// coordinates to draw; the scalars let the UI be honest about the trade-off.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Route {
    pub polyline: Vec<LatLon>,
    /// Total walking distance in metres.
    pub length_m: f64,
    /// Mean exposure along the route, 0..1 (fraction under surveillance).
    pub mean_exposure: f64,
}

/// The one long-lived object the app holds. Construct it once from map data,
/// then call `plan` as often as you like. Immutable after construction, so it
/// is cheap to share across threads (hence `Arc` + `uniffi::Object`).
#[derive(uniffi::Object)]
pub struct Router {
    graph: Graph,
    cameras: CameraIndex,
    places: PlaceIndex,
    coords: HashMap<u64, (f64, f64)>,
}

#[uniffi::export]
impl Router {
    /// Build a router from an OSM extract on disk (a Berlin `.osm.pbf`).
    ///
    /// This does the ingest, graph build, and the one-off exposure scoring
    /// pass, then keeps the scored graph in memory. Do it off the UI thread.
    #[uniffi::constructor]
    pub fn from_pbf(pbf_path: String) -> Result<Arc<Self>, RouteError> {
        Ok(Arc::new(Self::assemble(build_parts_from_pbf(&pbf_path)?)))
    }

    /// Build a router, reading a cached scored graph if one is valid.
    ///
    /// Preferred over [`Router::from_pbf`] on device: the exposure pass over
    /// every edge is a several-second cost otherwise paid on every cold start,
    /// even though the bundled extract never changes. On a cache hit that pass
    /// is skipped entirely; on a miss (no cache, a stale one, or a rebuilt
    /// extract) it falls back to the PBF and writes a fresh cache for next
    /// time. `cache_path` is a writable app-private location (e.g. beside the
    /// extract in `filesDir`). A read or write failure is never fatal — the
    /// worst case is simply the old, uncached behaviour.
    #[uniffi::constructor]
    pub fn open(pbf_path: String, cache_path: String) -> Result<Arc<Self>, RouteError> {
        let fp = source_fingerprint(&pbf_path);

        // Cache hit: reload the scored parts and skip the exposure pass.
        if let Some(fp) = fp {
            if let Ok(file) = std::fs::File::open(&cache_path) {
                let mut reader = std::io::BufReader::new(file);
                if let Ok(parts) = cache::read(&mut reader, fp) {
                    return Ok(Arc::new(Self::assemble(parts)));
                }
            }
        }

        // Miss: build from the PBF, then best-effort write the cache so the
        // next launch hits. A write failure just leaves us uncached.
        let parts = build_parts_from_pbf(&pbf_path)?;
        if let Some(fp) = fp {
            write_cache(&cache_path, &parts, fp);
        }
        Ok(Arc::new(Self::assemble(parts)))
    }

    /// Plan a route. `lambda` is the paranoia dial:
    ///   * 0.0  → shortest path, ignore cameras
    ///   * ~1–3 → sensible avoidance
    ///   * >5   → will take big detours to dodge lenses
    pub fn plan(&self, start: LatLon, end: LatLon, lambda: f64) -> Result<Route, RouteError> {
        let start_id = self
            .graph
            .nearest_node(start.lat, start.lon)
            .ok_or(RouteError::NoNearbyNode)?;
        let goal_id = self
            .graph
            .nearest_node(end.lat, end.lon)
            .ok_or(RouteError::NoNearbyNode)?;

        let path = self
            .graph
            .plan(start_id, goal_id, lambda.max(0.0))
            .ok_or(RouteError::Unreachable)?;

        let polyline = path
            .node_ids
            .iter()
            .filter_map(|id| self.coords.get(id))
            .map(|&(lat, lon)| LatLon { lat, lon })
            .collect();

        Ok(Route {
            polyline,
            length_m: path.length_m,
            mean_exposure: path.mean_exposure,
        })
    }

    /// Cameras within `radius_m` of a point — for the map's "cameras nearby"
    /// layer. Returns coordinates + kind so the UI can pick an icon.
    pub fn cameras_near(&self, at: LatLon, radius_m: f64) -> Vec<Camera> {
        self.cameras.near(at.lat, at.lon, radius_m)
    }

    /// How many cameras the core knows about (for a status line / honesty note).
    pub fn camera_count(&self) -> u64 {
        self.cameras.len() as u64
    }

    /// Street, locality and station names matching `query`, best first.
    ///
    /// There is no geocoder behind this — the names come from the bundled
    /// extract, so searching leaks nothing and works with the radio off.
    pub fn search_places(&self, query: String, limit: u32) -> Vec<Place> {
        self.places.search(&query, limit as usize)
    }

    /// How many searchable names were found in the extract.
    pub fn place_count(&self) -> u64 {
        self.places.len() as u64
    }
}

impl Router {
    /// Assemble the live router from its flat parts, rebuilding every index and
    /// grid. Shared by the PBF and cache paths so both produce an identical
    /// router — the cache stores only these parts, never the derived indices.
    /// Not on the UniFFI surface (it takes internal types); the constructors
    /// above are the exported entry points.
    fn assemble(parts: cache::Parts) -> Self {
        let cache::Parts {
            nodes,
            edges,
            cameras,
            places,
        } = parts;
        let coords: HashMap<u64, (f64, f64)> =
            nodes.iter().map(|n| (n.id, (n.lat, n.lon))).collect();
        Self {
            graph: Graph::new(nodes, edges),
            cameras: CameraIndex::new(cameras),
            places: PlaceIndex::new(places),
            coords,
        }
    }
}

/// Do the ingest, graph build and the one-off exposure scoring pass, yielding
/// the flat parts a `Router` is assembled from. This is the expensive path —
/// the exposure pass walks every edge — and the whole point of the cache is to
/// avoid running it on every launch. Kept free-standing so both constructors
/// and the cache-writing path share exactly one build.
fn build_parts_from_pbf(pbf_path: &str) -> Result<cache::Parts, RouteError> {
    let load = |e: osm::OsmError| RouteError::LoadFailed {
        reason: e.to_string(),
    };
    let cameras = osm::load_cameras(pbf_path).map_err(load)?;
    let network = osm::load_network(pbf_path).map_err(load)?;
    let (nodes, mut edges, places) = (network.nodes, network.edges, network.places);

    let coords: HashMap<u64, (f64, f64)> = nodes.iter().map(|n| (n.id, (n.lat, n.lon))).collect();

    // The expensive part, done once: attach exposure to every edge. Scored
    // against a throwaway index so the raw camera list can be cached as-is.
    let index = CameraIndex::new(cameras.clone());
    exposure::score_edges(&mut edges, &index, |id| {
        *coords.get(&id).unwrap_or(&(0.0, 0.0))
    });

    Ok(cache::Parts {
        nodes,
        edges,
        cameras,
        places,
    })
}

/// Fingerprint of the source extract for cache validation, or `None` if it
/// can't be stat'd (in which case caching is simply skipped). Uses file size
/// and last-modified time — see [`cache::fingerprint`].
fn source_fingerprint(pbf_path: &str) -> Option<u64> {
    let meta = std::fs::metadata(pbf_path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(cache::fingerprint(meta.len(), mtime))
}

/// Best-effort cache write: to a temp file then rename, so an interrupted write
/// never leaves a truncated file that would read back as valid-looking. Any
/// failure is swallowed — the cache is an optimisation, never required.
fn write_cache(cache_path: &str, parts: &cache::Parts, fingerprint: u64) {
    let tmp = format!("{cache_path}.part");
    let ok = (|| -> std::io::Result<()> {
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        cache::write(&mut writer, parts, fingerprint)?;
        writer.into_inner()?.sync_all()?;
        std::fs::rename(&tmp, cache_path)
    })()
    .is_ok();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
    }
}
