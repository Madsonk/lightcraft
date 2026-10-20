//! Private series workbench: metadata grouping and a read-only, cancellable colour plan.
//! Source decoding and measurements run off the UI thread. Apply validates every snapshot,
//! leaves the reference intact, and commits ordinary develop settings in one undo step.
use crate::cmd::{CommandSpec, always, bad, bool_or, cmd, has_selection};
use crate::media::{SourceLevel, SourceRef};
use crate::{Result, Session};
use lightcraft_catalog::{Op, Photo, PhotoId};
use lightcraft_develop::{DevelopSettings, SettingsGroup, WbMode};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

const LIMIT: usize = 1000;
const C: &str = "series.plan";
#[derive(Default)]
pub struct Workbench {
    state: Option<Arc<Mutex<State>>>,
}
struct Input {
    photo: Arc<Photo>,
    source: SourceRef,
    info: lightcraft_pipeline::SourceInfo,
    stamp: Option<(u64, Option<std::time::SystemTime>)>,
}
#[derive(Clone)]
struct Measure {
    exposure: f64,
    temp: f64,
    tint: f64,
    neutral: bool,
}
struct Row {
    photo: Arc<Photo>,
    proposed: Option<DevelopSettings>,
    report: Value,
    stamp: Option<(u64, Option<std::time::SystemTime>)>,
}
struct State {
    generation: u64,
    reference: Arc<Photo>,
    queue: VecDeque<Input>,
    rows: Vec<Row>,
    total: usize,
    measured: Option<Measure>,
    reference_wb: (f64, f64),
    running: bool,
    paused: bool,
    applied: bool,
    exposure: bool,
    wb: bool,
    look: bool,
    fatal: Option<String>,
}
fn lock(s: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
    s.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
impl Drop for Workbench {
    fn drop(&mut self) {
        if let Some(s) = &self.state {
            lock(s).paused = true;
        }
    }
}
fn source_stamp(photo: &Photo) -> Option<(u64, Option<std::time::SystemTime>)> {
    match &photo.source {
        lightcraft_catalog::Source::File { path } => std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok())),
        _ => None,
    }
}
impl Workbench {
    /// Temporary look for a before/after render; never changes selection or the catalogue.
    pub fn proposal(&self, id: PhotoId) -> Option<DevelopSettings> {
        let state = self.state.as_ref()?;
        lock(state).rows.iter().find(|row| row.photo.id == id)?.proposed.clone()
    }
    pub fn status(&self) -> Value {
        let Some(s) = &self.state else { return json!({"state":"idle"}) };
        let s = lock(s);
        json!({"state": if s.running {"running"} else if s.applied {"applied"} else if s.paused {"paused"} else {"ready"},
            "done":s.rows.len(),"total":s.total,"reference":s.reference.id.0,"error":s.fatal,
            "photos":s.rows.iter().map(|r|r.report.clone()).collect::<Vec<_>>(),
            "method":"native-statistics", "warning":"Review scene and lighting changes; estimates are not a calibrated AI model."})
    }
}
fn bounded_targets(s: &Session, p: &Value, c: &str) -> Result<Vec<PhotoId>> {
    let ids = s.checked_targets(c, p)?;
    if ids.len() > LIMIT {
        return Err(bad(c, "select at most 1000 photos per series"));
    }
    Ok(ids)
}
fn groups(s: &Session, p: &Value) -> Result<Value> {
    let mut photos: Vec<_> = bounded_targets(s, p, "series.groups")?.into_iter().filter_map(|id| s.catalog.photo(id).cloned()).collect();
    photos.sort_by_key(|p| (p.captured.as_deref().and_then(lightcraft_catalog::dates::iso_seconds), p.id));
    let mut out: Vec<Vec<u64>> = Vec::new();
    let mut previous: Option<Arc<Photo>> = None;
    for photo in photos {
        let same = previous.as_ref().is_some_and(|prev| {
            let time = prev
                .captured
                .as_deref()
                .and_then(lightcraft_catalog::dates::iso_seconds)
                .zip(photo.captured.as_deref().and_then(lightcraft_catalog::dates::iso_seconds))
                .is_some_and(|(a, b)| b.saturating_sub(a) <= 900 && b >= a);
            let places = prev.meta.location.trim().eq_ignore_ascii_case(photo.meta.location.trim())
                && prev.meta.city.trim().eq_ignore_ascii_case(photo.meta.city.trim());
            let gps = match (prev.meta.gps, photo.meta.gps) {
                (Some((a, b)), Some((c, d))) => [a, b, c, d].iter().all(|v| v.is_finite()) && (a - c).abs() < 0.002 && (b - d).abs() < 0.002,
                _ => true,
            };
            time && places && gps
        });
        if same {
            if let Some(g) = out.last_mut() {
                g.push(photo.id.0);
            }
        } else {
            out.push(vec![photo.id.0]);
        }
        previous = Some(photo);
    }
    Ok(
        json!({"groups":out,"gapSeconds":900,"method":"time-and-place","warning":"Missing times stay separate. Review lighting changes before harmonising."}),
    )
}
fn measure(input: &Input) -> std::result::Result<(Measure, lightcraft_pipeline::SourceInfo), String> {
    let decoded = input.source.load_source()?;
    if source_stamp(&input.photo) != input.stamp {
        return Err("source changed during analysis; create a new preview".into());
    }
    let info = decoded.info.clone().unwrap_or_else(|| input.info.clone());
    let a = lightcraft_pipeline::auto::auto_tone(&decoded.image, &info, &input.photo.develop);
    let (temp, tint) = lightcraft_pipeline::auto::auto_wb(&decoded.image, &info);
    let valid = decoded
        .image
        .data
        .iter()
        .filter(|c| {
            let max = c[0].max(c[1]).max(c[2]);
            let min = c[0].min(c[1]).min(c[2]);
            max.is_finite() && min.is_finite() && max > 0.02 && max < 0.9 && (max - min) / max < 0.35
        })
        .count();
    let neutral = valid >= 32 && valid.saturating_mul(100) >= decoded.image.data.len();
    Ok((Measure { exposure: a.exposure, temp, tint, neutral }, info))
}
#[cfg(not(target_arch = "wasm32"))]
fn launch(state: Arc<Mutex<State>>) -> std::result::Result<(), String> {
    {
        let mut s = lock(&state);
        if s.running {
            return Err("a series analysis is already running".into());
        }
        s.running = true;
        s.paused = false;
    }
    let worker = state.clone();
    let spawn=std::thread::Builder::new().name("series-plan".into()).spawn(move||{
        let outcome=crate::guard::catch("series analysis",|| {
            loop {
                let next={let mut s=lock(&worker);if s.paused {None}else{s.queue.pop_front()}};
                let Some(mut input)=next else {break};
                input.stamp=source_stamp(&input.photo);
                let result=measure(&input);
                let mut s=lock(&worker);
                match result {
                    Err(error)=>{
                        if input.photo.id==s.reference.id{s.fatal=Some(error.clone());s.queue.clear();}
                        s.rows.push(Row{stamp:input.stamp,photo:input.photo.clone(),proposed:None,report:json!({"id":input.photo.id.0,"name":input.photo.file_name,"error":error})});
                    }
                    Ok((m,info))=>{
                        if input.photo.id==s.reference.id {
                            s.reference_wb=lightcraft_pipeline::auto::current_wb(&info,&input.photo.develop);
                            s.measured=Some(m.clone());
                            s.rows.push(Row{stamp:input.stamp,photo:input.photo.clone(),proposed:None,report:json!({"id":input.photo.id.0,"name":input.photo.file_name,"reference":true})});
                            continue;
                        }
                        let Some(reference)=s.measured.clone() else {s.fatal=Some("reference measurement unavailable".into());break};
                        let mut d=(*input.photo.develop).clone();
                        if s.look {
                            let partial=lightcraft_develop::extract_groups(&s.reference.develop,&[SettingsGroup::Profile,SettingsGroup::ToneCurve,SettingsGroup::Color,SettingsGroup::ColorMixer,SettingsGroup::ColorGrading]);
                            d=lightcraft_develop::apply_partial(&d,&partial,1.0);
                        }
                        let delta=(m.exposure-reference.exposure).clamp(-2.0,2.0);
                        if s.exposure {d.light.exposure=(s.reference.develop.light.exposure+delta).clamp(-5.0,5.0);}
                        let reliable=m.neutral&&reference.neutral;
                        if s.wb&&reliable {
                            // Preserve the reference's creative WB offset in reciprocal kelvin, not by copying Kelvin.
                            let shift=(1e6/m.temp-1e6/reference.temp).clamp(-100.0,100.0);
                            d.wb.temp=(1e6/(1e6/s.reference_wb.0+shift).clamp(20.0,500.0)).clamp(2000.0,50000.0);
                            d.wb.tint=(s.reference_wb.1+(m.tint-reference.tint).clamp(-50.0,50.0)).clamp(-150.0,150.0);
                            d.wb.mode=WbMode::Custom;
                        }
                        let report=json!({"id":input.photo.id.0,"name":input.photo.file_name,"exposure":d.light.exposure,"temperature":d.wb.temp,"tint":d.wb.tint,"wbEstimated":s.wb&&reliable,"review":delta.abs()>=1.99||!reliable,"beforeHash":input.photo.develop.hash64()});
                        s.rows.push(Row{stamp:input.stamp,photo:input.photo,proposed:Some(d),report});
                    }
                }
            }
        });
        let mut s=lock(&worker);s.running=false;if let Err(e)=outcome{s.fatal=Some(e.to_string());}
    });
    if let Err(e) = spawn {
        let mut s = lock(&state);
        s.running = false;
        return Err(e.to_string());
    }
    Ok(())
}
#[cfg(target_arch = "wasm32")]
fn launch(_: Arc<Mutex<State>>) -> std::result::Result<(), String> {
    Err("series analysis requires the native app".into())
}
fn start(s: &mut Session, p: &Value) -> Result<Value> {
    if s.series.state.as_ref().is_some_and(|x| lock(x).running) {
        return Err(bad(C, "pause the running plan before starting another"));
    }
    let ids = bounded_targets(s, p, C)?;
    let reference = p.get("reference").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad(C, "no reference photo"))?;
    if !ids.contains(&reference) {
        return Err(bad(C, "the reference must be in the explicit selection"));
    }
    let photo = s.catalog.photo(reference).cloned().ok_or_else(|| bad(C, "no reference"))?;
    let mut queue = VecDeque::new();
    for id in std::iter::once(reference).chain(ids.iter().copied().filter(|id| *id != reference)) {
        let ph = s.catalog.photo(id).cloned().ok_or_else(|| bad(C, "photo disappeared"))?;
        let source = s.media.source_ref_uncached(&ph, SourceLevel::Thumb);
        let info = s.source_info(id);
        let stamp = None;
        queue.push_back(Input { photo: ph, source, info, stamp });
    }
    let state = Arc::new(Mutex::new(State {
        generation: s.library_generation(),
        reference: photo,
        queue,
        rows: Vec::new(),
        total: ids.len(),
        measured: None,
        reference_wb: (6500.0, 0.0),
        running: false,
        paused: false,
        applied: false,
        exposure: bool_or(p, "exposure", true),
        wb: bool_or(p, "whiteBalance", true),
        look: bool_or(p, "look", true),
        fatal: None,
    }));
    launch(state.clone()).map_err(|e| bad(C, e))?;
    s.series.state = Some(state);
    Ok(s.series.status())
}
fn apply(s: &mut Session) -> Result<Value> {
    let shared = s.series.state.clone().ok_or_else(|| bad("series.apply", "no preview plan"))?;
    let state = lock(&shared);
    if state.running || state.paused || !state.queue.is_empty() || state.fatal.is_some() || state.applied {
        return Err(bad("series.apply", "finish and review an unapplied plan first"));
    }
    if state.generation != s.library_generation() {
        return Err(bad("series.apply", "the library changed; create a new preview"));
    }
    // Validate all snapshots before making any edit, including the reference.
    for row in &state.rows {
        let current = s.catalog.photo(row.photo.id).ok_or_else(|| bad("series.apply", "a planned photo disappeared"))?;
        if current.deleted
            || source_stamp(current) != row.stamp
            || current.source != row.photo.source
            || current.develop != row.photo.develop
            || current.meta != row.photo.meta
            || current.captured != row.photo.captured
        {
            return Err(bad("series.apply", "photos changed since preview; create a new preview"));
        }
    }
    let mut ops = Vec::new();
    for row in &state.rows {
        if let Some(d) = &row.proposed {
            ops.extend(s.develop_op(row.photo.id, d.clone(), "Harmonise series"));
        }
    }
    let changed = ops.len();
    drop(state);
    if changed > 0 {
        s.commit("Harmonise series", Op::Batch { ops })?;
    }
    lock(&shared).applied = true;
    Ok(json!({"changed":changed}))
}
pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "series.groups","Find photo series",[],None,"{ids?} -> metadata groups; read-only, missing times are isolated",has_selection,|s,p|groups(s,p)),
        cmd!(query "series.plan","Preview series harmonisation",[],None,"{ids?,reference?,exposure?:true,whiteBalance?:true,look?:true} -> background preview only; at most 1000 photos",has_selection,|s,p|start(s,p)),
        cmd!(query "series.status","Series progress",[],None,"{} -> progress and proposals; read-only",always,|s,_|Ok(s.series.status())),
        cmd!(query "series.pause","Pause series analysis",[],None,"{} -> pause after current photo, no edits",always,|s,_|{if let Some(st)=&s.series.state{lock(st).paused=true;}Ok(s.series.status())}),
        cmd!(query "series.resume","Resume series analysis",[],None,"{} -> continue remaining photos without duplicate measurements",always,|s,_|{let st=s.series.state.clone().ok_or_else(||bad("series.resume","no paused plan"))?;{let st=lock(&st);if !st.paused||st.running||st.applied||st.fatal.is_some(){return Err(bad("series.resume","no resumable plan"));}}launch(st).map_err(|e|bad("series.resume",e))?;Ok(s.series.status())}),
        cmd!(
            "series.apply",
            "Apply reviewed series",
            [],
            None,
            "{} -> validates snapshots, leaves reference intact; one undo step",
            always,
            |s, _| apply(s)
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(s: &Session) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while s.series.status()["state"] == "running" {
            assert!(std::time::Instant::now() < deadline, "analysis timeout");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    #[test]
    fn series_preview_is_read_only_apply_is_single_undo_and_reference_is_untouched() {
        let mut s = Session::with_demo();
        let ids = s.visible_cloned();
        let ids = &ids[..3];
        s.execute("library.select", &json!({"ids":ids,"active":ids[0]})).unwrap();
        let before = s.catalog.to_snapshot();
        let undo = s.undo.len();
        s.execute("series.plan", &json!({"ids":ids})).unwrap();
        wait(&s);
        assert_eq!(s.catalog.to_snapshot(), before);
        assert_eq!(s.undo.len(), undo);
        let reference = s.develop_of(ids[0]).unwrap();
        let report = s.execute("series.apply", &json!({})).unwrap();
        assert!(report["changed"].as_u64().unwrap() > 0);
        assert_eq!(s.develop_of(ids[0]).unwrap(), reference);
        assert_eq!(s.undo.len(), undo + 1);
        assert!(s.execute("series.apply", &json!({})).is_err());
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(s.catalog.to_snapshot(), before);
    }
    #[test]
    fn series_stale_plan_refuses_every_edit() {
        let mut s = Session::with_demo();
        let ids = s.visible_cloned();
        let ids = &ids[..2];
        s.execute("series.plan", &json!({"ids":ids,"reference":ids[0]})).unwrap();
        wait(&s);
        s.set_develop(
            ids[1],
            DevelopSettings { light: lightcraft_develop::Light { exposure: 0.5, ..Default::default() }, ..Default::default() },
            "manual",
        )
        .unwrap();
        let before = s.catalog.to_snapshot();
        assert!(s.execute("series.apply", &json!({})).is_err());
        assert_eq!(s.catalog.to_snapshot(), before);
    }
    #[test]
    fn series_invalid_targets_and_missing_dates_are_safe() {
        let mut s = Session::with_demo();
        let ids = s.visible_cloned();
        for p in [json!({"ids":[]}), json!({"ids":["oops"]}), json!({"ids":[ids[0]],"reference":ids[1]})] {
            assert!(s.execute("series.plan", &p).is_err());
        }
        let groups = s.execute("series.groups", &json!({"ids":[ids[0],ids[1]]})).unwrap();
        assert_eq!(groups["groups"].as_array().unwrap().iter().map(|g| g.as_array().unwrap().len()).sum::<usize>(), 2);
    }
    #[test]
    fn series_pause_resume_keeps_one_result_per_photo() {
        let mut s = Session::with_demo();
        let ids = s.visible_cloned();
        let ids = &ids[..8];
        s.execute("series.plan", &json!({"ids":ids,"reference":ids[0]})).unwrap();
        s.execute("series.pause", &json!({})).unwrap();
        wait(&s);
        let before = s.catalog.to_snapshot();
        s.execute("series.resume", &json!({})).unwrap();
        wait(&s);
        let report = s.series.status();
        let rows = report["photos"].as_array().unwrap();
        assert_eq!(rows.len(), ids.len());
        let unique: std::collections::HashSet<_> = rows.iter().map(|r| r["id"].as_u64().unwrap()).collect();
        assert_eq!(unique.len(), ids.len());
        assert_eq!(s.catalog.to_snapshot(), before);
    }
}
