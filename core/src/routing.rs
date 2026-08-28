//! Camera-aware routing.
//!
//! The graph is the usual node/edge adjacency built from OSM ways. The twist
//! is the edge weight:
//!
//! ```text
//!     weight(edge) = length_m * (1 + lambda * exposure)
//! ```
//!
//! * `lambda == 0`  → ordinary shortest path, cameras ignored.
//! * `lambda` large → the router will take long detours to shave exposure.
//!
//! Because the penalty is multiplicative on length, it composes cleanly and
//! stays in metres-equivalent units, which keeps the A* heuristic admissible
//! (straight-line distance never overestimates true cost when `lambda >= 0`).

use crate::exposure::{Edge, Node};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// Grid cell size for the nearest-node index, in metres. ~100 m keeps buckets
/// small at Berlin node density while letting the ring search stop early.
const NODE_CELL_M: f64 = 100.0;

/// Give up snapping a tap to the network beyond this many rings (~3 km) —
/// the caller surfaces that as "no graph node near the given point".
const MAX_RING: i32 = 30;

const M_PER_DEG_LAT: f64 = 111_320.0;

/// The routable graph. `nodes` is indexed by a compact internal index; the
/// `id_to_index` map bridges OSM/node ids to those indices. `adjacency[i]`
/// holds outgoing edges from node `i`. `node_grid` buckets node indices into
/// ~100 m cells for nearest-node snapping.
pub struct Graph {
    nodes: Vec<Node>,
    id_to_index: HashMap<u64, usize>,
    adjacency: Vec<Vec<Edge>>,
    node_grid: HashMap<(i32, i32), Vec<u32>>,
    cell_lat_deg: f64,
    cell_lon_deg: f64,
}

impl Graph {
    pub fn new(nodes: Vec<Node>, edges: Vec<Edge>) -> Self {
        let mut id_to_index = HashMap::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            id_to_index.insert(n.id, i);
        }
        let mut adjacency = vec![Vec::new(); nodes.len()];
        for e in edges {
            if let Some(&i) = id_to_index.get(&e.from) {
                adjacency[i].push(e);
            }
        }

        let mean_lat = if nodes.is_empty() {
            0.0
        } else {
            nodes.iter().map(|n| n.lat).sum::<f64>() / nodes.len() as f64
        };
        let cell_lat_deg = NODE_CELL_M / M_PER_DEG_LAT;
        let cell_lon_deg = NODE_CELL_M / (M_PER_DEG_LAT * mean_lat.to_radians().cos().max(0.01));
        let mut node_grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, n) in nodes.iter().enumerate() {
            let key = (
                (n.lat / cell_lat_deg).floor() as i32,
                (n.lon / cell_lon_deg).floor() as i32,
            );
            node_grid.entry(key).or_default().push(i as u32);
        }

        Self {
            nodes,
            id_to_index,
            adjacency,
            node_grid,
            cell_lat_deg,
            cell_lon_deg,
        }
    }

    fn coord(&self, index: usize) -> (f64, f64) {
        let n = &self.nodes[index];
        (n.lat, n.lon)
    }

    /// Nearest graph node to an arbitrary coordinate — used to snap the user's
    /// start/end taps onto the network.
    ///
    /// Expanding ring search over the ~100 m grid: scan rings of cells outward
    /// and stop once every unscanned ring is provably farther than the best
    /// hit ((k-1) * cell size is a lower bound on distance to ring k). Returns
    /// `None` when nothing lies within ~MAX_RING cells (a tap far outside the
    /// mapped area).
    pub fn nearest_node(&self, lat: f64, lon: f64) -> Option<u64> {
        let ci = (lat / self.cell_lat_deg).floor() as i32;
        let cj = (lon / self.cell_lon_deg).floor() as i32;

        let mut best: Option<(u64, f64)> = None;
        for ring in 0..=MAX_RING {
            if let Some((_, d)) = best {
                if f64::from(ring - 1) * NODE_CELL_M > d {
                    break;
                }
            }
            for di in -ring..=ring {
                for dj in -ring..=ring {
                    // Only the ring's border cells; the interior was already
                    // scanned in previous iterations.
                    if di.abs() != ring && dj.abs() != ring {
                        continue;
                    }
                    let Some(bucket) = self.node_grid.get(&(ci + di, cj + dj)) else {
                        continue;
                    };
                    for &i in bucket {
                        let n = &self.nodes[i as usize];
                        let d = crate::camera::haversine_m(lat, lon, n.lat, n.lon);
                        if best.is_none_or(|(_, bd)| d < bd) {
                            best = Some((n.id, d));
                        }
                    }
                }
            }
        }
        best.map(|(id, _)| id)
    }

    /// Plan a route from `start_id` to `goal_id` with paranoia `lambda`.
    /// Returns the ordered node ids of the path, or `None` if unreachable.
    pub fn plan(&self, start_id: u64, goal_id: u64, lambda: f64) -> Option<PlannedPath> {
        let start = *self.id_to_index.get(&start_id)?;
        let goal = *self.id_to_index.get(&goal_id)?;
        let (glat, glon) = self.coord(goal);

        let n = self.nodes.len();
        let mut g_cost = vec![f64::INFINITY; n];
        let mut came_from = vec![usize::MAX; n];
        g_cost[start] = 0.0;

        let mut open = BinaryHeap::new();
        open.push(Frontier {
            index: start,
            f_cost: 0.0,
        });

        while let Some(Frontier { index, .. }) = open.pop() {
            if index == goal {
                return Some(self.reconstruct(&came_from, start, goal));
            }
            for edge in &self.adjacency[index] {
                let Some(&next) = self.id_to_index.get(&edge.to) else {
                    continue;
                };
                let step = edge.length_m * (1.0 + lambda * edge.exposure);
                let tentative = g_cost[index] + step;
                if tentative < g_cost[next] {
                    came_from[next] = index;
                    g_cost[next] = tentative;
                    let (nlat, nlon) = self.coord(next);
                    // Admissible heuristic: straight-line metres to goal, which
                    // can never exceed real cost while lambda >= 0.
                    let h = crate::camera::haversine_m(nlat, nlon, glat, glon);
                    open.push(Frontier {
                        index: next,
                        f_cost: tentative + h,
                    });
                }
            }
        }
        None
    }

    fn reconstruct(&self, came_from: &[usize], start: usize, goal: usize) -> PlannedPath {
        let mut path = vec![goal];
        let mut cur = goal;
        while cur != start {
            cur = came_from[cur];
            path.push(cur);
        }
        path.reverse();

        // Recompute plain distance and mean exposure over the chosen path so
        // the UI can show the honest "you traded X metres for Y% less camera".
        let mut length_m = 0.0;
        let mut exposure_len = 0.0;
        for w in path.windows(2) {
            if let Some(edge) = self.edge_between(w[0], w[1]) {
                length_m += edge.length_m;
                exposure_len += edge.length_m * edge.exposure;
            }
        }
        let node_ids = path.iter().map(|&i| self.nodes[i].id).collect();
        PlannedPath {
            node_ids,
            length_m,
            mean_exposure: if length_m > 0.0 {
                exposure_len / length_m
            } else {
                0.0
            },
        }
    }

    fn edge_between(&self, a: usize, b: usize) -> Option<&Edge> {
        let b_id = self.nodes[b].id;
        self.adjacency[a].iter().find(|e| e.to == b_id)
    }
}

/// Result of a plan: the path plus the numbers the UI needs to be honest about
/// the trade-off it made.
pub struct PlannedPath {
    pub node_ids: Vec<u64>,
    pub length_m: f64,
    pub mean_exposure: f64,
}

/// Min-heap frontier entry (BinaryHeap is a max-heap, so we invert Ord).
struct Frontier {
    index: usize,
    f_cost: f64,
}

impl PartialEq for Frontier {
    fn eq(&self, other: &Self) -> bool {
        self.f_cost == other.f_cost
    }
}
impl Eq for Frontier {}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse so the smallest f_cost is popped first.
        other
            .f_cost
            .partial_cmp(&self.f_cost)
            .unwrap_or(Ordering::Equal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Three-node line: A --100m-- B --100m-- C, plus a direct A--C that is
    // longer (250 m) but camera-free, while A-B-C passes a camera on B-C.
    fn toy_graph() -> Graph {
        let nodes = vec![
            Node {
                id: 1,
                lat: 0.0,
                lon: 0.0,
            },
            Node {
                id: 2,
                lat: 0.0,
                lon: 0.001,
            },
            Node {
                id: 3,
                lat: 0.0,
                lon: 0.002,
            },
        ];
        let edges = vec![
            Edge {
                from: 1,
                to: 2,
                length_m: 100.0,
                exposure: 0.0,
            },
            Edge {
                from: 2,
                to: 3,
                length_m: 100.0,
                exposure: 1.0,
            }, // watched
            Edge {
                from: 1,
                to: 3,
                length_m: 250.0,
                exposure: 0.0,
            }, // long, clean
        ];
        Graph::new(nodes, edges)
    }

    #[test]
    fn nearest_node_snaps_to_closest() {
        let g = toy_graph();
        assert_eq!(g.nearest_node(0.0, 0.0001), Some(1));
        assert_eq!(g.nearest_node(0.0, 0.0019), Some(3));
        // Far outside the mapped area: refuse rather than snap absurdly.
        assert_eq!(g.nearest_node(10.0, 10.0), None);
    }

    #[test]
    fn lambda_zero_takes_shortest() {
        let g = toy_graph();
        let p = g.plan(1, 3, 0.0).unwrap();
        // 100 + 100 = 200 < 250, so the watched route wins when we don't care.
        assert_eq!(p.node_ids, vec![1, 2, 3]);
    }

    #[test]
    fn high_lambda_avoids_camera() {
        let g = toy_graph();
        let p = g.plan(1, 3, 5.0).unwrap();
        // Watched route now costs 100 + 100*(1+5) = 700 vs 250 clean.
        assert_eq!(p.node_ids, vec![1, 3]);
    }
}
