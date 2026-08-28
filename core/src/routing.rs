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

/// The routable graph. `nodes` is indexed by a compact internal index; the
/// `id_to_index` map bridges OSM/node ids to those indices. `adjacency[i]`
/// holds outgoing edges from node `i`.
pub struct Graph {
    nodes: Vec<Node>,
    id_to_index: HashMap<u64, usize>,
    adjacency: Vec<Vec<Edge>>,
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
        Self {
            nodes,
            id_to_index,
            adjacency,
        }
    }

    fn coord(&self, index: usize) -> (f64, f64) {
        let n = &self.nodes[index];
        (n.lat, n.lon)
    }

    /// Nearest graph node to an arbitrary coordinate — used to snap the user's
    /// start/end taps onto the network. Linear scan for now; swap for the same
    /// spatial index as the cameras when it matters.
    pub fn nearest_node(&self, lat: f64, lon: f64) -> Option<u64> {
        self.nodes
            .iter()
            .map(|n| (n.id, crate::camera::haversine_m(lat, lon, n.lat, n.lon)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
            .map(|(id, _)| id)
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
                return Some(self.reconstruct(&came_from, start, goal, g_cost[goal]));
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

    fn reconstruct(
        &self,
        came_from: &[usize],
        start: usize,
        goal: usize,
        weighted_cost: f64,
    ) -> PlannedPath {
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
            weighted_cost,
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
    pub weighted_cost: f64,
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
            Node { id: 1, lat: 0.0, lon: 0.0 },
            Node { id: 2, lat: 0.0, lon: 0.001 },
            Node { id: 3, lat: 0.0, lon: 0.002 },
        ];
        let edges = vec![
            Edge { from: 1, to: 2, length_m: 100.0, exposure: 0.0 },
            Edge { from: 2, to: 3, length_m: 100.0, exposure: 1.0 }, // watched
            Edge { from: 1, to: 3, length_m: 250.0, exposure: 0.0 }, // long, clean
        ];
        Graph::new(nodes, edges)
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
