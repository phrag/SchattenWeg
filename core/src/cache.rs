//! On-disk cache of the fully-built, exposure-scored graph.
//!
//! Building a `Router` from a PBF does the ingest, the graph build, and — the
//! slow part — the exposure pass that walks every edge sampling camera
//! coverage. On the real Berlin extract that pass is a several-second cost paid
//! on *every* cold start, even though its input (the bundled extract) never
//! changes between launches. This module lets the first build write the scored
//! result to disk so later launches read it back instead of re-deriving it.
//!
//! What is cached is only the four flat inputs the `Router` is assembled from —
//! `nodes`, the *scored* `edges`, `cameras` and `places`. Every index and grid
//! (`Graph` adjacency, the camera grid, the place search list, the id→coord
//! map) is cheap to rebuild from those with the same constructors the PBF path
//! uses, so none of it is serialised. The edge exposure — the expensive bit —
//! lives in `edges`, so reloading skips the pass entirely.
//!
//! Format: a small hand-rolled little-endian binary blob. No serialisation
//! dependency, and full control over versioning and validation — the header
//! carries a format/logic version and a fingerprint of the source extract, and
//! any mismatch (or any malformed byte) makes the reader fail so the caller
//! rebuilds from the PBF. The file is app-private (`filesDir`), so this
//! validation is about staleness and truncation, not untrusted input.

use crate::camera::{Camera, CameraKind};
use crate::exposure::{Edge, Node};
use crate::places::{Place, PlaceKind};
use std::io::{self, Read, Write};

/// Magic bytes at the head of every cache file ("SchattenWeG Cache").
const MAGIC: &[u8; 4] = b"SWGC";

/// Format/logic version. **Bump this whenever the on-disk layout changes OR
/// the scoring/geometry logic that produces the cached values changes** (e.g. a
/// change to `camera.rs` coverage, the `defaults` table, or the exposure
/// sampling in `exposure.rs`). The cached edge exposures are only as current as
/// the code that wrote them; bumping the version invalidates every old cache so
/// a stale score can never outlive the logic that produced it.
const VERSION: u32 = 1;

/// The four flat vectors a `Router` is assembled from. This is exactly what the
/// PBF ingest produces (with edges already scored) and exactly what the cache
/// round-trips.
#[derive(Debug)]
pub struct Parts {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub cameras: Vec<Camera>,
    pub places: Vec<Place>,
}

/// Serialise `parts` to `w`, tagged with `fingerprint` (see [`fingerprint`]).
pub fn write<W: Write>(w: &mut W, parts: &Parts, fingerprint: u64) -> io::Result<()> {
    w.write_all(MAGIC)?;
    write_u32(w, VERSION)?;
    write_u64(w, fingerprint)?;

    write_u64(w, parts.nodes.len() as u64)?;
    for n in &parts.nodes {
        write_u64(w, n.id)?;
        write_f64(w, n.lat)?;
        write_f64(w, n.lon)?;
    }

    write_u64(w, parts.edges.len() as u64)?;
    for e in &parts.edges {
        write_u64(w, e.from)?;
        write_u64(w, e.to)?;
        write_f64(w, e.length_m)?;
        write_f64(w, e.exposure)?;
    }

    write_u64(w, parts.cameras.len() as u64)?;
    for c in &parts.cameras {
        write_i64(w, c.osm_id)?;
        write_f64(w, c.lat)?;
        write_f64(w, c.lon)?;
        w.write_all(&[camera_kind_tag(c.kind)])?;
        match c.direction_deg {
            Some(d) => {
                w.write_all(&[1])?;
                write_f64(w, d)?;
            }
            None => w.write_all(&[0])?,
        }
        write_f64(w, c.half_fov_deg)?;
        write_f64(w, c.range_m)?;
    }

    write_u64(w, parts.places.len() as u64)?;
    for p in &parts.places {
        let bytes = p.name.as_bytes();
        write_u32(w, bytes.len() as u32)?;
        w.write_all(bytes)?;
        w.write_all(&[place_kind_tag(p.kind)])?;
        write_f64(w, p.lat)?;
        write_f64(w, p.lon)?;
    }

    Ok(())
}

/// Read parts back, requiring the header magic and `VERSION` to match and the
/// stored fingerprint to equal `expected_fingerprint`. Any mismatch, short
/// read, or out-of-range tag is an `InvalidData` error so the caller falls back
/// to rebuilding from the PBF.
pub fn read<R: Read>(r: &mut R, expected_fingerprint: u64) -> io::Result<Parts> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(bad("not a Schattenweg cache file"));
    }
    if read_u32(r)? != VERSION {
        return Err(bad("cache version mismatch"));
    }
    if read_u64(r)? != expected_fingerprint {
        return Err(bad("cache is stale (source extract changed)"));
    }

    let node_count = read_len(r)?;
    let mut nodes = Vec::with_capacity(node_count.min(MAX_PREALLOC));
    for _ in 0..node_count {
        nodes.push(Node {
            id: read_u64(r)?,
            lat: read_f64(r)?,
            lon: read_f64(r)?,
        });
    }

    let edge_count = read_len(r)?;
    let mut edges = Vec::with_capacity(edge_count.min(MAX_PREALLOC));
    for _ in 0..edge_count {
        edges.push(Edge {
            from: read_u64(r)?,
            to: read_u64(r)?,
            length_m: read_f64(r)?,
            exposure: read_f64(r)?,
        });
    }

    let camera_count = read_len(r)?;
    let mut cameras = Vec::with_capacity(camera_count.min(MAX_PREALLOC));
    for _ in 0..camera_count {
        let osm_id = read_i64(r)?;
        let lat = read_f64(r)?;
        let lon = read_f64(r)?;
        let kind = camera_kind_from_tag(read_u8(r)?)?;
        let direction_deg = match read_u8(r)? {
            0 => None,
            1 => Some(read_f64(r)?),
            _ => return Err(bad("invalid camera direction tag")),
        };
        let half_fov_deg = read_f64(r)?;
        let range_m = read_f64(r)?;
        cameras.push(Camera {
            osm_id,
            lat,
            lon,
            kind,
            direction_deg,
            half_fov_deg,
            range_m,
        });
    }

    let place_count = read_len(r)?;
    let mut places = Vec::with_capacity(place_count.min(MAX_PREALLOC));
    for _ in 0..place_count {
        let name_len = read_u32(r)? as usize;
        let mut buf = vec![0u8; name_len];
        r.read_exact(&mut buf)?;
        let name = String::from_utf8(buf).map_err(|_| bad("place name is not UTF-8"))?;
        let kind = place_kind_from_tag(read_u8(r)?)?;
        places.push(Place {
            name,
            kind,
            lat: read_f64(r)?,
            lon: read_f64(r)?,
        });
    }

    Ok(Parts {
        nodes,
        edges,
        cameras,
        places,
    })
}

/// A cheap identifier for a source extract: its byte length mixed with its last
/// modification time. If the bundled extract is replaced (a new Geofabrik cut),
/// either changes and the old cache is rejected. Not a content hash — hashing a
/// ~100 MB PBF on every launch would eat the very time the cache saves — but
/// enough to catch the only realistic staleness cause: a rebuilt asset.
pub fn fingerprint(len: u64, mtime_secs: i64) -> u64 {
    // A tiny splitmix-style mix so len and mtime both spread across all bits.
    let mut x = len ^ ((mtime_secs as u64).rotate_left(32));
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Cap on up-front allocation from a length field, so a corrupt count can't ask
/// for a huge `Vec` before the reader hits the truncated data and errors. The
/// vectors still grow past this if the data is genuinely that large.
const MAX_PREALLOC: usize = 1 << 20;

fn read_len<R: Read>(r: &mut R) -> io::Result<usize> {
    Ok(read_u64(r)? as usize)
}

fn camera_kind_tag(k: CameraKind) -> u8 {
    match k {
        CameraKind::Fixed => 0,
        CameraKind::Dome => 1,
        CameraKind::Panning => 2,
        CameraKind::Unknown => 3,
    }
}

fn camera_kind_from_tag(t: u8) -> io::Result<CameraKind> {
    Ok(match t {
        0 => CameraKind::Fixed,
        1 => CameraKind::Dome,
        2 => CameraKind::Panning,
        3 => CameraKind::Unknown,
        _ => return Err(bad("invalid camera kind tag")),
    })
}

fn place_kind_tag(k: PlaceKind) -> u8 {
    match k {
        PlaceKind::Street => 0,
        PlaceKind::Locality => 1,
        PlaceKind::Station => 2,
    }
}

fn place_kind_from_tag(t: u8) -> io::Result<PlaceKind> {
    Ok(match t {
        0 => PlaceKind::Street,
        1 => PlaceKind::Locality,
        2 => PlaceKind::Station,
        _ => return Err(bad("invalid place kind tag")),
    })
}

fn bad(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn write_u32<W: Write>(w: &mut W, v: u32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn write_u64<W: Write>(w: &mut W, v: u64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn write_i64<W: Write>(w: &mut W, v: i64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}
fn write_f64<W: Write>(w: &mut W, v: f64) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn read_u8<R: Read>(r: &mut R) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}
fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn read_i64<R: Read>(r: &mut R) -> io::Result<i64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(i64::from_le_bytes(b))
}
fn read_f64<R: Read>(r: &mut R) -> io::Result<f64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(f64::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Parts {
        Parts {
            nodes: vec![
                Node {
                    id: 1,
                    lat: 52.5,
                    lon: 13.4,
                },
                Node {
                    id: 2,
                    lat: 52.51,
                    lon: 13.41,
                },
            ],
            edges: vec![Edge {
                from: 1,
                to: 2,
                length_m: 123.4,
                exposure: 0.42,
            }],
            cameras: vec![
                Camera {
                    osm_id: -7,
                    lat: 52.505,
                    lon: 13.405,
                    kind: CameraKind::Fixed,
                    direction_deg: Some(90.0),
                    half_fov_deg: 30.0,
                    range_m: 25.0,
                },
                Camera {
                    osm_id: 9,
                    lat: 52.506,
                    lon: 13.406,
                    kind: CameraKind::Dome,
                    direction_deg: None,
                    half_fov_deg: 0.0,
                    range_m: 15.0,
                },
            ],
            places: vec![Place {
                name: "Alexanderplatz".to_string(),
                kind: PlaceKind::Locality,
                lat: 52.521,
                lon: 13.413,
            }],
        }
    }

    fn assert_round_trips(parts: &Parts) {
        let mut buf = Vec::new();
        write(&mut buf, parts, 0xdead_beef).unwrap();
        let back = read(&mut &buf[..], 0xdead_beef).unwrap();

        assert_eq!(back.nodes.len(), parts.nodes.len());
        for (a, b) in back.nodes.iter().zip(&parts.nodes) {
            assert_eq!((a.id, a.lat, a.lon), (b.id, b.lat, b.lon));
        }
        assert_eq!(back.edges.len(), parts.edges.len());
        for (a, b) in back.edges.iter().zip(&parts.edges) {
            assert_eq!(
                (a.from, a.to, a.length_m, a.exposure),
                (b.from, b.to, b.length_m, b.exposure)
            );
        }
        assert_eq!(back.cameras.len(), parts.cameras.len());
        for (a, b) in back.cameras.iter().zip(&parts.cameras) {
            assert_eq!(a.osm_id, b.osm_id);
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.direction_deg, b.direction_deg);
            assert_eq!(a.half_fov_deg, b.half_fov_deg);
            assert_eq!(a.range_m, b.range_m);
        }
        assert_eq!(back.places.len(), parts.places.len());
        for (a, b) in back.places.iter().zip(&parts.places) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.kind, b.kind);
        }
    }

    #[test]
    fn round_trips_all_fields() {
        assert_round_trips(&sample());
    }

    #[test]
    fn round_trips_when_empty() {
        assert_round_trips(&Parts {
            nodes: vec![],
            edges: vec![],
            cameras: vec![],
            places: vec![],
        });
    }

    #[test]
    fn rejects_a_different_fingerprint() {
        let mut buf = Vec::new();
        write(&mut buf, &sample(), 1).unwrap();
        let err = read(&mut &buf[..], 2).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_a_bad_magic() {
        let mut buf = Vec::new();
        write(&mut buf, &sample(), 1).unwrap();
        buf[0] = b'X';
        assert!(read(&mut &buf[..], 1).is_err());
    }

    #[test]
    fn rejects_truncation() {
        let mut buf = Vec::new();
        write(&mut buf, &sample(), 1).unwrap();
        buf.truncate(buf.len() / 2);
        assert!(read(&mut &buf[..], 1).is_err());
    }

    #[test]
    fn fingerprint_reacts_to_len_and_mtime() {
        let base = fingerprint(100, 1000);
        assert_ne!(base, fingerprint(101, 1000));
        assert_ne!(base, fingerprint(100, 1001));
    }
}
