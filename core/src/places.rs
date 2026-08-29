//! Searchable names: streets, localities and transit stops.
//!
//! The app has no network, so there is no geocoder to call — the names come
//! from the same bundled extract everything else does, collected during the
//! graph build (see `osm::load_network`).

/// What kind of thing a search result is, so the UI can say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PlaceKind {
    /// A named road.
    Street,
    /// A city, borough, suburb, quarter or neighbourhood.
    Locality,
    /// A railway station or halt.
    Station,
}

/// One searchable name and where it is.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Place {
    pub name: String,
    pub kind: PlaceKind,
    pub lat: f64,
    pub lon: f64,
}

/// Case- and accent-insensitive name lookup over the bundled places.
///
/// Deliberately a linear scan: Berlin yields a few thousand names, and at that
/// size a scan costs well under a millisecond — far cheaper than keeping a
/// prefix structure in sync, and it supports matching mid-word ("platz").
pub struct PlaceIndex {
    /// Places paired with their pre-folded search key, so a keystroke does not
    /// re-normalise every name.
    entries: Vec<(String, Place)>,
}

impl PlaceIndex {
    pub fn new(mut places: Vec<Place>) -> Self {
        // One entry per name+kind: OSM splits a street into many ways, and
        // localities can be tagged on both a node and a boundary.
        places.sort_by(|a, b| a.name.cmp(&b.name));
        places.dedup_by(|a, b| a.name == b.name && a.kind == b.kind);

        let entries = places
            .into_iter()
            .map(|p| (fold(&p.name), p))
            .collect::<Vec<_>>();
        Self { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Best matches for `query`, most relevant first.
    ///
    /// Ranking, in order: a name that starts with the query beats one that
    /// merely contains it, then shorter names beat longer ones (so "Alexander-
    /// platz" outranks "Alexanderplatz Bahnhof Nordseite"), then alphabetical
    /// so results never reshuffle between identical queries.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Place> {
        let needle = fold(query);
        if needle.is_empty() {
            return Vec::new();
        }

        let mut hits: Vec<(u8, usize, &Place)> = self
            .entries
            .iter()
            .filter_map(|(key, place)| {
                let rank = if key.starts_with(&needle) {
                    0
                } else if key.contains(&needle) {
                    1
                } else {
                    return None;
                };
                Some((rank, place.name.chars().count(), place))
            })
            .collect();

        hits.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.cmp(&b.1))
                .then_with(|| a.2.name.cmp(&b.2.name))
        });
        hits.into_iter()
            .take(limit)
            .map(|(_, _, p)| p.clone())
            .collect()
    }
}

/// Lowercase, and map the German (and neighbouring) letters a user is likely
/// to type without accents. Someone typing "muller" should find "Müller".
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.trim().to_lowercase().chars() {
        match ch {
            'ä' | 'à' | 'á' | 'â' | 'ã' | 'å' => out.push('a'),
            'ö' | 'ò' | 'ó' | 'ô' | 'õ' | 'ø' => out.push('o'),
            'ü' | 'ù' | 'ú' | 'û' => out.push('u'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'í' | 'ì' | 'î' | 'ï' => out.push('i'),
            'ç' => out.push('c'),
            'ñ' => out.push('n'),
            'ß' => out.push_str("ss"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(name: &str, kind: PlaceKind) -> Place {
        Place {
            name: name.to_string(),
            kind,
            lat: 52.52,
            lon: 13.40,
        }
    }

    fn index() -> PlaceIndex {
        PlaceIndex::new(vec![
            place("Alexanderplatz", PlaceKind::Locality),
            place("Alexanderplatz Bahnhof Nordseite", PlaceKind::Station),
            place("Karl-Marx-Allee", PlaceKind::Street),
            place("Müllerstraße", PlaceKind::Street),
            place("Prenzlauer Berg", PlaceKind::Locality),
        ])
    }

    #[test]
    fn prefix_matches_outrank_contains() {
        let hits = index().search("alexander", 10);
        assert_eq!(hits[0].name, "Alexanderplatz");
        assert_eq!(hits[1].name, "Alexanderplatz Bahnhof Nordseite");
    }

    #[test]
    fn matches_mid_word() {
        let hits = index().search("marx", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Karl-Marx-Allee");
    }

    #[test]
    fn folds_umlauts_and_eszett() {
        // Typed without the umlaut, and without the ß.
        assert_eq!(index().search("muller", 10)[0].name, "Müllerstraße");
        assert_eq!(index().search("müllerstrasse", 10)[0].name, "Müllerstraße");
    }

    #[test]
    fn ignores_case_and_surrounding_space() {
        assert_eq!(
            index().search("  PRENZLAUER ", 10)[0].name,
            "Prenzlauer Berg"
        );
    }

    #[test]
    fn empty_query_returns_nothing() {
        assert!(index().search("   ", 10).is_empty());
    }

    #[test]
    fn limit_is_respected() {
        assert_eq!(index().search("a", 2).len(), 2);
    }

    #[test]
    fn duplicate_street_names_collapse() {
        // OSM splits a street into many ways; each would arrive separately.
        let idx = PlaceIndex::new(vec![
            place("Hauptstraße", PlaceKind::Street),
            place("Hauptstraße", PlaceKind::Street),
            place("Hauptstraße", PlaceKind::Street),
        ]);
        assert_eq!(idx.len(), 1);
    }
}
