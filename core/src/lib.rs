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
        let load = |e: osm::OsmError| RouteError::LoadFailed {
            reason: e.to_string(),
        };
        let cameras = CameraIndex::new(osm::load_cameras(&pbf_path).map_err(load)?);
        let network = osm::load_network(&pbf_path).map_err(load)?;
        let (nodes, mut edges) = (network.nodes, network.edges);
        let places = PlaceIndex::new(network.places);

        let coords: HashMap<u64, (f64, f64)> =
            nodes.iter().map(|n| (n.id, (n.lat, n.lon))).collect();

        // The expensive part, done once: attach exposure to every edge.
        exposure::score_edges(&mut edges, &cameras, |id| {
            *coords.get(&id).unwrap_or(&(0.0, 0.0))
        });

        Ok(Arc::new(Self {
            graph: Graph::new(nodes, edges),
            cameras,
            places,
            coords,
        }))
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
