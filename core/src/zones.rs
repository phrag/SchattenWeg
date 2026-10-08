//! Police AI-video zones: where Berlin authorities run, or have announced,
//! AI behaviour detection on CCTV ("KI-gestützter Videoschutz").
//!
//! The table lives here, in the core, so there is exactly one copy: the
//! exposure pass reads it to score edges and the UI reads the same records to
//! draw and describe the circles. A picture that disagrees with the model is
//! worse than none (see CLAUDE.md §4), and one table cannot disagree with
//! itself.
//!
//! **Geometry is crude on purpose.** The authorities publish names, not
//! boundaries or camera positions (the Senate declined to release them:
//! Drucksache 19/26970), so each zone is a circle around a landmark. The real
//! edge will be the signage promised at every entrance to a protected area;
//! replace the circles with surveyed outlines once it exists.
//!
//! **How a zone enters routing:** inside a zone every sample point counts as
//! watched, exactly like a point inside a camera's field of view. So a zone
//! raises an edge's exposure by the share of the edge inside the circle, and
//! the ordinary `length * (1 + λ * exposure)` weight does the rest.
//!
//! Sources: Berlin Abgeordnetenhaus Drucksache 19/26970 (Senate answer of
//! 6 Sep 2026) and the Polizei Berlin press release of 24 Sep 2026.

/// Metres per degree of latitude (and of longitude at the equator).
const M_PER_DEG_LAT: f64 = 111_320.0;

/// When the table below was last checked against its sources.
pub const AS_OF: &str = "October 2026";

/// How far along a zone is. Both count for routing today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ZoneStatus {
    /// Cameras are being installed or calibrated; no live alerts yet.
    Commissioning,
    /// Announced with a date, not yet started.
    Planned,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AiZone {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// Illustrative radius, not an official boundary.
    pub radius_m: f64,
    pub status: ZoneStatus,
    /// What is known about timing.
    pub note: String,
    /// What the system is said to detect there.
    pub detects: String,
}

const POLICE_DETECTS: &str = "Violence (hitting, kicking) and people lying motionless. The \
screen stays black until the software flags an event; an officer then decides.";

const OBJEKTSCHUTZ_DETECTS: &str = "Entering restricted areas, crossing virtual lines, \
climbing fences, vandalism and abandoned objects.";

const BUILDING_NOTE: &str = "Building-protection pilot. Test phase planned for Q2 2027.";

fn zone(
    id: &str,
    name: &str,
    (lat, lon): (f64, f64),
    radius_m: f64,
    status: ZoneStatus,
    note: &str,
    detects: &str,
) -> AiZone {
    AiZone {
        id: id.into(),
        name: name.into(),
        lat,
        lon,
        radius_m,
        status,
        note: note.into(),
        detects: detects.into(),
    }
}

/// The zone table. Editing it changes [`signature`], which invalidates any
/// cached scored graph, so stale scores cannot outlive an edit.
pub fn table() -> Vec<AiZone> {
    use ZoneStatus::{Commissioning, Planned};
    vec![
        zone(
            "kotti",
            "Kottbusser Tor",
            (52.4990, 13.4178),
            150.0,
            Commissioning,
            "About 30 cameras. Set-up and calibration began 24 Sep 2026, roughly four \
weeks, then a test phase. Not yet issuing alerts.",
            POLICE_DETECTS,
        ),
        zone(
            "warschauer",
            "Warschauer Brücke",
            (52.5060, 13.4498),
            150.0,
            Planned,
            "Planned for completion by the end of 2026. No firm date.",
            POLICE_DETECTS,
        ),
        zone(
            "alex",
            "Alexanderplatz",
            (52.5219, 13.4132),
            250.0,
            Planned,
            "Planned for 2027.",
            POLICE_DETECTS,
        ),
        zone(
            "goerli",
            "Görlitzer Park / Wrangelkiez",
            (52.4966, 13.4350),
            350.0,
            Planned,
            "Planned for 2027. Reported to cover the park's busy entrances and \
crossings, not the whole park.",
            POLICE_DETECTS,
        ),
        zone(
            "rathaus",
            "Rotes Rathaus",
            (52.5186, 13.4083),
            70.0,
            Planned,
            BUILDING_NOTE,
            OBJEKTSCHUTZ_DETECTS,
        ),
        zone(
            "stadthaus",
            "Altes Stadthaus (Innenverwaltung)",
            (52.5153, 13.4154),
            70.0,
            Planned,
            BUILDING_NOTE,
            OBJEKTSCHUTZ_DETECTS,
        ),
        zone(
            "juedmus",
            "Jüdisches Museum",
            (52.5020, 13.3954),
            70.0,
            Planned,
            BUILDING_NOTE,
            OBJEKTSCHUTZ_DETECTS,
        ),
    ]
}

/// A hash of everything about the zones that changes a score: ids and
/// geometry. Mixed into the cache fingerprint so editing the table rebuilds
/// the cache without anyone having to remember to bump a version.
pub fn signature(zones: &[AiZone]) -> u64 {
    // FNV-1a: tiny, dependency-free, and plenty for change detection.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for z in zones {
        eat(z.id.as_bytes());
        eat(&z.lat.to_bits().to_le_bytes());
        eat(&z.lon.to_bits().to_le_bytes());
        eat(&z.radius_m.to_bits().to_le_bytes());
    }
    h
}

/// Point-in-circle test over a handful of zones. Zones are few, so a linear
/// scan with a cheap latitude reject beats any spatial structure.
pub struct ZoneIndex {
    zones: Vec<Circle>,
}

struct Circle {
    lat: f64,
    lon: f64,
    r2: f64,
    /// Metres per degree of longitude at this zone's latitude.
    m_per_deg_lon: f64,
    /// The radius as degrees of latitude: a cheap reject before any multiply.
    radius_deg_lat: f64,
}

impl ZoneIndex {
    pub fn new(zones: &[AiZone]) -> Self {
        Self {
            zones: zones
                .iter()
                .map(|z| Circle {
                    lat: z.lat,
                    lon: z.lon,
                    r2: z.radius_m * z.radius_m,
                    m_per_deg_lon: M_PER_DEG_LAT * z.lat.to_radians().cos().max(0.01),
                    radius_deg_lat: z.radius_m / M_PER_DEG_LAT,
                })
                .collect(),
        }
    }

    /// No zones at all: what the unit tests of pure camera scoring use.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self { zones: Vec::new() }
    }

    /// True if the point lies inside any zone. Flat-earth distance, which is
    /// exact enough over a few hundred metres.
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        self.zones.iter().any(|c| {
            let dlat = lat - c.lat;
            if dlat.abs() > c.radius_deg_lat {
                return false;
            }
            let dy = dlat * M_PER_DEG_LAT;
            let dx = (lon - c.lon) * c.m_per_deg_lon;
            dx * dx + dy * dy <= c.r2
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::haversine_m;
    use std::collections::HashSet;

    #[test]
    fn table_is_sane() {
        let zones = table();
        let ids: HashSet<_> = zones.iter().map(|z| z.id.as_str()).collect();
        assert_eq!(ids.len(), zones.len(), "zone ids must be unique");
        for z in &zones {
            assert!((20.0..=1_000.0).contains(&z.radius_m), "{} radius", z.id);
            // Roughly inside Berlin; catches a swapped lat/lon.
            assert!((52.3..52.7).contains(&z.lat), "{} lat", z.id);
            assert!((13.0..13.8).contains(&z.lon), "{} lon", z.id);
            assert!(!z.name.is_empty() && !z.note.is_empty() && !z.detects.is_empty());
        }
    }

    #[test]
    fn contains_agrees_with_haversine_away_from_the_edge() {
        let zones = table();
        let idx = ZoneIndex::new(&zones);
        for z in &zones {
            for i in -30..=30 {
                for j in -30..=30 {
                    let lat = z.lat + f64::from(i) * 0.0002;
                    let lon = z.lon + f64::from(j) * 0.0003;
                    let d = haversine_m(z.lat, z.lon, lat, lon);
                    // The two distance models differ by a hair at the rim; only
                    // demand agreement where it is not a knife-edge. Another
                    // zone may overlap, so only the "inside" side is exact.
                    if d < z.radius_m * 0.98 {
                        assert!(idx.contains(lat, lon), "{} inside at {d}", z.id);
                    }
                }
            }
        }
        // And far from every zone, nothing matches.
        assert!(!idx.contains(52.60, 13.20));
    }

    #[test]
    fn empty_index_contains_nothing() {
        assert!(!ZoneIndex::empty().contains(52.4990, 13.4178));
    }

    #[test]
    fn signature_reacts_to_geometry_but_not_prose() {
        let base = table();
        let sig = signature(&base);

        let mut moved = base.clone();
        moved[0].lat += 0.0001;
        assert_ne!(sig, signature(&moved));

        let mut grown = base.clone();
        grown[1].radius_m += 1.0;
        assert_ne!(sig, signature(&grown));

        let mut dropped = base.clone();
        dropped.pop();
        assert_ne!(sig, signature(&dropped));

        // A reworded note does not change any score, so it must not discard a
        // perfectly good cache.
        let mut reworded = base.clone();
        reworded[0].note.push_str(" (updated)");
        assert_eq!(sig, signature(&reworded));
    }
}
