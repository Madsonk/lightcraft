//! Places in the library: a photo's place is the hierarchical keyword `Places|Country|Region|City` it carries (set by
//! `places.geotag`, `places.name`…); a photo with a GPS position but no such keyword has the place its position
//! resolves to (offline reverse geocoding, feature `places`), so the Place filter and the place list work before
//! anything is written. A place keyword the user chose always wins over the GPS.
//!
//! [`Filter::place`](crate::Filter) matches on the hierarchy (`Places|France` finds `Places|France|New Aquitaine|Poitiers`);
//! the special value [`NONE`] matches the photos with no place at all.

use std::collections::HashMap;

use serde::Serialize;

use crate::{Catalog, Filter, Photo, keywords};

/// The first level of every place keyword.
pub const ROOT: &str = "Places";
/// `Filter::place` value for "photos with no place".
pub const NONE: &str = "none";

/// The photo's GPS position when it is a usable one (the (0, 0) cameras write without a fix is not).
pub fn valid_gps(p: &Photo) -> Option<(f64, f64)> {
    p.meta.gps.filter(|(a, o)| a.is_finite() && o.is_finite() && a.abs() <= 90.0 && o.abs() <= 360.0 && !(*a == 0.0 && *o == 0.0))
}

/// A keyword of the `Places` branch.
pub fn is_place_keyword(k: &str) -> bool {
    keywords::is_under(k, ROOT)
}

/// The place the photo's GPS position resolves to, as a keyword (`None` without GPS, without the places data or far
/// from any place). Uses the default radius.
pub fn gps_place(p: &Photo) -> Option<String> {
    #[cfg(feature = "places")]
    {
        let (lat, lon) = valid_gps(p)?;
        lightcraft_places::place_at(lat, lon, lightcraft_places::DEFAULT_MAX_KM).map(|pl| pl.keyword())
    }
    #[cfg(not(feature = "places"))]
    {
        let _ = p;
        None
    }
}

/// Where a photo is: its `Places` keywords as written, else the place its GPS resolves to. The flag says the place
/// came from the GPS only.
pub fn photo_places(p: &Photo) -> (Vec<String>, bool) {
    let kw: Vec<String> = p.meta.keywords.iter().filter(|k| is_place_keyword(k)).cloned().collect();
    if !kw.is_empty() {
        return (kw, false);
    }
    match gps_place(p) {
        Some(g) => (vec![g], true),
        None => (Vec::new(), false),
    }
}

/// Whether the photo is in `place` (a keyword path, or [`NONE`]).
pub fn matches(p: &Photo, place: &str) -> bool {
    let place = place.trim();
    let (places, _) = photo_places(p);
    if place.eq_ignore_ascii_case(NONE) {
        return places.is_empty();
    }
    places.iter().any(|k| keywords::is_under(k, place))
}

/// One level of the place tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceNode {
    pub name: String,
    /// The full keyword, `Places|France|New Aquitaine`.
    pub path: String,
    /// Photos in this place or below it.
    pub count: usize,
    /// Of those, photos placed only by their GPS (no place keyword yet).
    pub from_gps: usize,
    pub children: Vec<PlaceNode>,
}

/// The places of a set of photos.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceSummary {
    /// The tree below `Places`, biggest first.
    pub tree: Vec<PlaceNode>,
    pub total: usize,
    /// Photos with at least one place.
    pub placed: usize,
    /// Of those, photos placed only by their GPS.
    pub from_gps: usize,
    /// Photos with no place at all.
    pub unplaced: usize,
    /// Photos with a GPS position and no place keyword (what `places.geotag` would tag).
    pub gps_untagged: usize,
}

impl PlaceSummary {
    /// The nodes depth first, with their depth (0 = a country).
    pub fn flatten(&self) -> Vec<(usize, &PlaceNode)> {
        fn go<'a>(n: &'a PlaceNode, depth: usize, out: &mut Vec<(usize, &'a PlaceNode)>) {
            out.push((depth, n));
            for c in &n.children {
                go(c, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        for n in &self.tree {
            go(n, 0, &mut out);
        }
        out
    }
}

struct Acc {
    name: String,
    path: String,
    count: usize,
    from_gps: usize,
    parent: Option<String>,
}

impl Catalog {
    /// The places among the photos `filter` lets through (its own `place` aside): what the Place picker and the
    /// Places view show next to each place, so each count is what picking it would leave in the grid.
    pub fn places_in(&self, filter: &Filter) -> PlaceSummary {
        let filter = Filter { place: None, ..filter.clone() };
        let mut levels: HashMap<String, Acc> = HashMap::new();
        let mut s = PlaceSummary::default();
        for p in self.photos().filter(|p| filter.matches(p, self)) {
            s.total += 1;
            let (places, gps_only) = photo_places(p);
            if valid_gps(p).is_some() && !p.meta.keywords.iter().any(|k| is_place_keyword(k)) {
                s.gps_untagged += 1;
            }
            if places.is_empty() {
                s.unplaced += 1;
                continue;
            }
            s.placed += 1;
            s.from_gps += usize::from(gps_only);
            let mut seen: Vec<String> = Vec::new();
            for k in &places {
                let mut path = String::new();
                let mut lower = String::new();
                let mut parent: Option<String> = None;
                for (i, part) in k.split(keywords::SEP).map(str::trim).filter(|x| !x.is_empty()).enumerate() {
                    if !path.is_empty() {
                        path.push(keywords::SEP);
                        lower.push(keywords::SEP);
                    }
                    path.push_str(part);
                    lower.push_str(&part.to_lowercase());
                    // the root itself is not a place
                    if i > 0 {
                        let acc = levels.entry(lower.clone()).or_insert_with(|| Acc {
                            name: part.to_string(),
                            path: path.clone(),
                            count: 0,
                            from_gps: 0,
                            parent: parent.clone(),
                        });
                        if !seen.contains(&lower) {
                            acc.count += 1;
                            acc.from_gps += usize::from(gps_only);
                            seen.push(lower.clone());
                        }
                        parent = Some(lower.clone());
                    }
                }
            }
        }
        // assemble the tree
        let mut children: HashMap<Option<String>, Vec<String>> = HashMap::new();
        for (key, a) in &levels {
            children.entry(a.parent.clone()).or_default().push(key.clone());
        }
        fn build(key: &str, levels: &HashMap<String, Acc>, children: &HashMap<Option<String>, Vec<String>>) -> Option<PlaceNode> {
            let a = levels.get(key)?;
            let mut kids: Vec<PlaceNode> =
                children.get(&Some(key.to_string())).map(|v| v.iter().filter_map(|c| build(c, levels, children)).collect()).unwrap_or_default();
            kids.sort_by(|x, y| y.count.cmp(&x.count).then_with(|| x.name.to_lowercase().cmp(&y.name.to_lowercase())));
            Some(PlaceNode { name: a.name.clone(), path: a.path.clone(), count: a.count, from_gps: a.from_gps, children: kids })
        }
        let mut tree: Vec<PlaceNode> =
            children.get(&None).map(|v| v.iter().filter_map(|k| build(k, &levels, &children)).collect()).unwrap_or_default();
        tree.sort_by(|x, y| y.count.cmp(&x.count).then_with(|| x.name.to_lowercase().cmp(&y.name.to_lowercase())));
        s.tree = tree;
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Op, Source};

    fn add(c: &mut Catalog, name: &str, kws: &[&str], gps: Option<(f64, f64)>) -> crate::PhotoId {
        let id = c.alloc_photo_id();
        let mut p = Photo::new(id, Source::File { path: format!("/x/{name}") }, name, "jpg", 10, 10, "2026-01-01T00:00:00");
        p.meta.keywords = kws.iter().map(|s| s.to_string()).collect();
        p.meta.gps = gps;
        c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        id
    }

    #[test]
    fn filter_summary_and_none() {
        let mut c = Catalog::new();
        let a = add(&mut c, "a", &["Places|France|New Aquitaine|Poitiers", "beach"], None);
        let b = add(&mut c, "b", &["Places|France|New Aquitaine|Niort"], None);
        let d = add(&mut c, "d", &["Places|Comoros|Grande Comore|Moroni", "Places|France"], None);
        let e = add(&mut c, "e", &["beach"], None);
        let q = |f: Filter| c.query(&f, &crate::Sort::default());
        let sorted = |mut v: Vec<crate::PhotoId>| {
            v.sort();
            v
        };
        assert_eq!(sorted(q(Filter { place: Some("Places|France".into()), ..Default::default() })), vec![a, b, d]);
        assert_eq!(q(Filter { place: Some("places|france|new aquitaine|poitiers".into()), ..Default::default() }), vec![a]);
        assert_eq!(q(Filter { place: Some("Places|Fra".into()), ..Default::default() }), Vec::<crate::PhotoId>::new(), "whole levels only");
        assert_eq!(q(Filter { place: Some(NONE.into()), ..Default::default() }), vec![e]);
        assert_eq!(q(Filter { place: Some("Places|France".into()), keyword: Some("beach".into()), ..Default::default() }), vec![a]);
        let s = c.places_in(&Filter::default());
        assert_eq!((s.total, s.placed, s.unplaced, s.from_gps), (4, 3, 1, 0));
        let flat: Vec<(usize, String, usize)> = s.flatten().into_iter().map(|(d, n)| (d, n.path.clone(), n.count)).collect();
        assert_eq!(flat[0], (0, "Places|France".to_string(), 3), "{flat:?}");
        assert!(flat.contains(&(2, "Places|France|New Aquitaine|Poitiers".to_string(), 1)));
        assert!(flat.contains(&(1, "Places|France|New Aquitaine".to_string(), 2)));
        // a photo counted once per place even with two keywords under it; the filter's own place is ignored
        let s = c.places_in(&Filter { place: Some("Places|Comoros".into()), ..Default::default() });
        assert_eq!(s.total, 4);
        let s = c.places_in(&Filter { keyword: Some("beach".into()), ..Default::default() });
        assert_eq!((s.total, s.placed), (2, 1));
    }

    #[test]
    fn filter_serialises_only_when_set() {
        let off = serde_json::to_value(Filter::default()).unwrap();
        assert!(off.get("place").is_none(), "{off}");
        let old: Filter = serde_json::from_value(serde_json::json!({"rating": 2})).unwrap();
        assert_eq!(old.place, None);
        let on: Filter = serde_json::from_value(serde_json::json!({"place": "Places|France"})).unwrap();
        assert_eq!(on.place.as_deref(), Some("Places|France"));
        assert!(on.describe().contains("place Places|France"), "{}", on.describe());
    }
}
