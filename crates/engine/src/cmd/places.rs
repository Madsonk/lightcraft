//! Places: `places.*` — offline reverse geocoding of GPS positions into `Places|Country|Region|City` keywords (with a
//! preview before anything is written), naming places by hand, suggestions for photos without GPS, and the place list.
//! See `crate::places` and `docs/places.md`.

use lightcraft_catalog::{Filter, MediaKind, PhotoId};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, f64_or, str_param};
use crate::places::{self, SuggestOptions, apply_places, count_by_place, geotag_plan, normalize_place};
use crate::{EngineError, Result, Session};

fn other(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}

fn round(v: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (v * f).round() / f
}

/// The photos a command works on: `ids` when given, else every photo of the library.
fn scope(s: &Session, p: &Value) -> Vec<PhotoId> {
    if p.get("ids").is_some() { s.targets(p) } else { s.catalog.photos().filter(|q| q.in_library()).map(|q| q.id).collect() }
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let st = s.places_settings();
    let data = {
        #[cfg(feature = "places")]
        {
            match lightcraft_places::database_status() {
                Ok((path, prov, n)) => json!({"installed": true, "path": path.display().to_string(), "places": n, "provenance": prov}),
                Err(e) => json!({"installed": false, "error": e, "envVar": lightcraft_places::DB_ENV, "file": lightcraft_places::DB_FILE}),
            }
        }
        #[cfg(not(feature = "places"))]
        {
            json!({"installed": false, "error": places::UNAVAILABLE})
        }
    };
    Ok(json!({
        "available": places::AVAILABLE,
        "data": data,
        "settings": st,
        "visualSuggestions": s.searcher.installed(),
        "attribution": "Place data from GeoNames (https://www.geonames.org/), CC BY 4.0.",
    }))
}

fn settings(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "places.settings";
    let mut st = s.places_settings();
    if let Some(v) = p.get("autoGeotag") {
        st.auto_geotag = v.as_bool().ok_or_else(|| bad(C, "autoGeotag is true or false"))?;
    }
    if let Some(v) = p.get("maxKm") {
        let km = v.as_f64().filter(|k| k.is_finite() && *k >= 1.0 && *k <= 500.0).ok_or_else(|| bad(C, "maxKm is a distance in km, 1 to 500"))?;
        st.max_km = km;
    }
    if p.get("autoGeotag").is_some() || p.get("maxKm").is_some() {
        s.set_places_settings(st)?;
    }
    Ok(json!(s.places_settings()))
}

fn lookup(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "places.lookup";
    let (Some(lat), Some(lon)) = (p.get("lat").and_then(Value::as_f64), p.get("lon").and_then(Value::as_f64)) else {
        return Err(bad(C, "`lat` and `lon` (degrees) are required"));
    };
    let km = p.get("maxKm").and_then(Value::as_f64).unwrap_or_else(|| s.places_settings().max_km);
    #[cfg(feature = "places")]
    if lightcraft_places::valid_position(lat, lon).is_none() {
        return Err(bad(C, "not a usable position (latitude -90..90, longitude -180..180, and not 0,0)"));
    }
    let r = places::resolve(lat, lon, km).map_err(other)?;
    #[cfg(feature = "places")]
    let nearby = lightcraft_places::database().map_or(Value::Null, |db| json!(db.nearby(lat, lon, 3)));
    #[cfg(not(feature = "places"))]
    let nearby = Value::Null;
    Ok(match r {
        Some(r) => json!({
            "found": true, "keyword": r.keyword, "level": r.level, "distanceKm": round(r.distance_km, 2),
            "country": r.country, "region": r.region, "city": r.city, "population": r.population, "nearest": nearby,
        }),
        None => json!({"found": false, "reason": "no place within range (open sea, a pole or a desert)", "nearest": nearby}),
    })
}

fn geotag(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "places.geotag";
    let apply = bool_or(p, "apply", false);
    let overwrite = bool_or(p, "overwrite", false);
    let km = match p.get("maxKm") {
        Some(v) => v.as_f64().filter(|k| k.is_finite() && *k >= 1.0 && *k <= 500.0).ok_or_else(|| bad(C, "maxKm is a distance in km, 1 to 500"))?,
        None => s.places_settings().max_km,
    };
    let limit = f64_or(p, "limit", 100.0).clamp(0.0, 10_000.0) as usize;
    let ids = scope(s, p);
    let t0 = std::time::Instant::now();
    let plan = geotag_plan(s, &ids, km, overwrite)?;
    let seconds = t0.elapsed().as_secs_f64();
    let counts = count_by_place(&plan.planned);
    let levels = {
        let mut m = std::collections::BTreeMap::new();
        for x in &plan.planned {
            *m.entry(x.place.level).or_insert(0usize) += 1;
        }
        m
    };
    let mut written = 0;
    if apply && !plan.planned.is_empty() {
        let assign: Vec<(PhotoId, Option<String>)> = plan.planned.iter().map(|x| (x.id, Some(x.place.keyword.clone()))).collect();
        written = apply_places(s, &format!("Geotag {} Photo{}", assign.len(), if assign.len() == 1 { "" } else { "s" }), &assign)?;
    }
    let photos: Vec<Value> = plan
        .planned
        .iter()
        .take(limit)
        .map(|x| {
            json!({
                "id": x.id.0,
                "fileName": s.catalog.photo(x.id).map(|q| q.file_name.clone()),
                "lat": round(x.lat, 5), "lon": round(x.lon, 5),
                "place": x.place.keyword, "level": x.place.level, "distanceKm": round(x.place.distance_km, 2),
            })
        })
        .collect();
    Ok(json!({
        "apply": apply,
        "applied": apply,
        "maxKm": km,
        "scanned": plan.scanned,
        "withoutGps": plan.no_gps,
        "alreadyPlaced": plan.already_placed,
        "resolved": plan.planned.len(),
        "unresolved": plan.unresolved.len(),
        "written": written,
        "wouldWrite": if apply { Value::Null } else { json!(plan.planned.len()) },
        "byLevel": levels,
        "places": counts.iter().map(|(k, n)| json!({"place": k, "count": n})).collect::<Vec<_>>(),
        "photos": photos,
        "unresolvedIds": plan.unresolved.iter().take(50).map(|(i, la, lo)| json!({"id": i.0, "lat": round(*la, 5), "lon": round(*lo, 5)})).collect::<Vec<_>>(),
        "seconds": round(seconds, 3),
        "photosPerSecond": if seconds > 0.0 { json!(round(plan.scanned as f64 / seconds, 0)) } else { Value::Null },
    }))
}

fn name(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "places.name";
    let place = str_param(p, "place")
        .and_then(normalize_place)
        .ok_or_else(|| bad(C, "`place` is required: \"Country|Region|City\" (or starting with Places|)"))?;
    let ids = s.targets(p);
    if ids.is_empty() {
        return Err(bad(C, "no photos selected"));
    }
    let n =
        apply_places(s, &format!("Place: {}", place.replacen("Places|", "", 1)), &ids.iter().map(|i| (*i, Some(place.clone()))).collect::<Vec<_>>())?;
    for i in &ids {
        s.places.suggestions.remove(i);
    }
    Ok(json!({"place": place, "changed": n}))
}

fn clear(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = s.targets(p);
    let n = apply_places(s, "Clear Place", &ids.iter().map(|i| (*i, None)).collect::<Vec<_>>())?;
    Ok(json!({"changed": n}))
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let filter = if bool_or(p, "inView", false) { s.filter.clone() } else { Filter::default() };
    let sum = s.catalog.places_in(&filter);
    let flat: Vec<Value> = sum
        .flatten()
        .into_iter()
        .map(|(d, n)| json!({"depth": d, "name": n.name, "path": n.path, "count": n.count, "fromGps": n.from_gps}))
        .collect();
    Ok(json!({
        "total": sum.total, "placed": sum.placed, "fromGps": sum.from_gps, "unplaced": sum.unplaced, "gpsUntagged": sum.gps_untagged,
        "places": flat, "tree": sum.tree,
    }))
}

fn suggest(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "places.suggest";
    let d = SuggestOptions { max_km: s.places_settings().max_km, ..Default::default() };
    let methods: Vec<String> = match p.get("methods").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        None => vec!["time".into(), "visual".into()],
    };
    if let Some(m) = methods.iter().find(|m| *m != "time" && *m != "visual") {
        return Err(bad(C, format!("methods are `time` and `visual`, not `{m}`")));
    }
    let o = SuggestOptions {
        window_secs: (f64_or(p, "windowMinutes", 30.0).clamp(1.0, 1440.0) * 60.0) as i64,
        same_camera: bool_or(p, "sameCamera", true),
        time: methods.iter().any(|m| m == "time"),
        visual: methods.iter().any(|m| m == "visual"),
        k: f64_or(p, "k", d.k as f64).clamp(1.0, 50.0) as usize,
        min_confidence: p.get("minConfidence").and_then(Value::as_f64).map(|v| v.clamp(0.0, 1.0) as f32),
        max_km: d.max_km,
    };
    let ids: Vec<PhotoId> = scope(s, p).into_iter().filter(|i| s.catalog.photo(*i).is_some_and(|q| q.kind != MediaKind::Video || o.time)).collect();
    places::suggest_for(s, &ids, &o)
}

fn confirm(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "places.confirm";
    let ids = s.targets(p);
    if ids.is_empty() {
        return Err(bad(C, "no photos selected"));
    }
    let explicit = match str_param(p, "place") {
        Some(t) => Some(normalize_place(t).ok_or_else(|| bad(C, "`place` is \"Country|Region|City\""))?),
        None => None,
    };
    let mut assign = Vec::new();
    let mut missing = Vec::new();
    for id in &ids {
        match explicit.clone().or_else(|| s.places.suggestions.get(id).map(|x| x.place.clone())) {
            Some(pl) => assign.push((*id, Some(pl))),
            None => missing.push(id.0),
        }
    }
    let n = apply_places(s, "Confirm Place", &assign)?;
    for (id, _) in &assign {
        s.places.suggestions.remove(id);
    }
    Ok(json!({"changed": n, "confirmed": assign.len(), "noSuggestion": missing}))
}

fn dismiss(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = s.targets(p);
    for i in &ids {
        s.places.suggestions.remove(i);
        s.places.dismissed.insert(*i);
    }
    Ok(json!({"dismissed": ids.len()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "places.status", "Places Status", [], None, "{} → {available, data: {installed, path, places, provenance | error}, settings: {autoGeotag, maxKm}, visualSuggestions, attribution} — whether the offline places data is installed (GeoNames, CC BY 4.0), the settings, and whether SigLIP can suggest places from looks", always, status),
        cmd!(
            "places.settings",
            "Places Settings",
            [],
            None,
            "{autoGeotag?: bool, maxKm?: km 1..500} — autoGeotag: give new photos their Places keyword when imported or added (off by default); maxKm: distance up to which the nearest place names the city (default 50; the region is named up to twice that, the country up to six times). Saved with the library → {autoGeotag, maxKm}",
            always,
            settings
        ),
        cmd!(query "places.lookup", "Look Up Place", [], None, "{lat, lon, maxKm?} → {found, keyword, level: city|region|country, distanceKm, country, region, city, population, nearest: [3 nearest places]} — offline reverse geocoding of a position", always, lookup),
        cmd!(
            "places.geotag",
            "Geotag Places",
            [],
            None,
            "{ids?, apply?: bool (default false = preview only, nothing is written), maxKm?, overwrite?: bool (replace existing Places keywords; default false), limit?: n photos listed (100)} — resolve the GPS of the photos (default: the whole library) to Places|Country|Region|City keywords. Without apply: → {scanned, withoutGps, alreadyPlaced, resolved, unresolved, wouldWrite, byLevel, places: [{place, count}], photos: [{id, place, level, distanceKm}], unresolvedIds}; with apply: the same plus written, in one undo step",
            always,
            geotag
        ),
        cmd!(
            "places.name",
            "Name Place",
            [],
            None,
            "{ids?, place: \"Country|Region|City\" (or Places|…)} — give the photos this place (replacing any other Places keyword), in one undo step → {place, changed}",
            always,
            name
        ),
        cmd!("places.clear", "Clear Place", [], None, "{ids?} — remove the Places keywords of the photos → {changed}", always, clear),
        cmd!(query "places.list", "List Places", [], None, "{inView?: bool (only the photos the filter lets through)} → {total, placed, fromGps, unplaced, gpsUntagged, places: [{depth, name, path, count, fromGps}], tree} — the places of the library with photo counts; a photo without a Places keyword counts in the place its GPS resolves to", always, list),
        cmd!(query "places.suggest", "Suggest Places", [], None, "{ids? (default: the whole library), methods?: [\"time\", \"visual\"], windowMinutes?: 30, sameCamera?: true, k?: 7 look-alikes, minConfidence?: 0.2 for time, 0.5 for visual} → {candidates, suggested, bySource, unindexed, suggestions: [{id, place, source, confidence, evidence, conflict}], notes} — places for photos with neither GPS nor place: `time` = the place of a photo of the same camera taken within windowMinutes; `visual` = the places of the photos most like it (SigLIP index: run search.index first). Only proposals: places.confirm writes them", always, suggest),
        cmd!(
            "places.confirm",
            "Confirm Place",
            [],
            None,
            "{ids?, place?} — write the place (default: the one last suggested for each photo) as a Places keyword → {changed, confirmed, noSuggestion}",
            always,
            confirm
        ),
        cmd!(
            "places.dismiss",
            "Dismiss Place Suggestion",
            [],
            None,
            "{ids?} — drop the suggestions for these photos and stop proposing for them this session → {dismissed}",
            always,
            dismiss
        ),
    ]
}
