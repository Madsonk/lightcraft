//! Videos: the library's videos with who is in them, the moments of one video (or the one moment
//! of a photo) to describe — scene, action, description — with the proposals beside what the user
//! typed, and the results of a "who + what" search. Everything is done by engine commands
//! (`video.list`, `video.moments`, `describe.*`, `video.frames`, `people.index`, `video.open`); this
//! panel only draws them and sends the user's gestures back.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{icon_button, register, text_button};

const PAD: f32 = 20.0;
/// Width of a moment's card on the sheet.
const CARD: f32 = 250.0;
/// Width of a video's card in the list.
const VCARD: f32 = 230.0;
/// Pictures decoded at once (the rest wait their turn, a frame at a time).
const MAX_TEXTURES: usize = 400;
/// Where Video Catcher keeps its database on this Mac, when nobody chose another.
pub const VIDEO_CATCHER_DB: &str = "Movies/VideoCatalog/libraries/e7f6a598ad826a8d/runtime/video-catcher.sqlite3";

/// The page the view shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    List,
    /// One video's (or photo's) moments.
    Sheet,
    /// The results of a who + what search.
    Search,
}

/// The pictures of moments as textures: decoded off the UI thread, kept for the most recently
/// used ones.
#[derive(Default)]
pub struct FrameTextures {
    textures: HashMap<String, (egui::TextureHandle, [usize; 2])>,
    order: VecDeque<String>,
    failed: HashSet<String>,
    pending: HashSet<String>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<(std::sync::mpsc::Sender<String>, std::sync::mpsc::Receiver<(String, Option<egui::ColorImage>)>)>,
}

#[cfg(not(target_arch = "wasm32"))]
fn decode_picture(path: &str) -> Option<egui::ColorImage> {
    let (w, h, px) = lightcraft_engine::video::picture_rgba(std::path::Path::new(path), 640)?;
    (px.len() == w * h * 4).then(|| egui::ColorImage::from_rgba_unmultiplied([w, h], &px))
}

impl FrameTextures {
    /// Pictures asked for and not decoded yet (the headless harness waits for them).
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// The picture at `path`, asked for if it is not there yet.
    pub fn get(&mut self, ctx: &egui::Context, path: &str) -> Option<(egui::TextureId, [usize; 2])> {
        self.receive(ctx);
        if let Some((t, size)) = self.textures.get(path) {
            return Some((t.id(), *size));
        }
        if self.failed.contains(path) || self.pending.contains(path) {
            return None;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.worker.is_none() {
                let (tx, rx) = std::sync::mpsc::channel::<String>();
                let (otx, orx) = std::sync::mpsc::channel();
                let repaint = ctx.clone();
                let started = std::thread::Builder::new().name("lc-video-pictures".into()).spawn(move || {
                    for p in rx {
                        let img = std::panic::catch_unwind(|| decode_picture(&p)).ok().flatten();
                        if otx.send((p, img)).is_err() {
                            break;
                        }
                        repaint.request_repaint();
                    }
                });
                if started.is_err() {
                    self.failed.insert(path.to_string());
                    return None;
                }
                self.worker = Some((tx, orx));
            }
            if let Some((tx, _)) = &self.worker
                && tx.send(path.to_string()).is_ok()
            {
                self.pending.insert(path.to_string());
            }
        }
        None
    }

    fn receive(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        while let Some(Ok((path, img))) = self.worker.as_ref().map(|(_, rx)| rx.try_recv()) {
            self.pending.remove(&path);
            match img {
                Some(img) => {
                    let size = img.size;
                    let tex = ctx.load_texture(format!("video-picture:{path}"), img, egui::TextureOptions::LINEAR);
                    self.textures.insert(path.clone(), (tex, size));
                    self.order.push_back(path);
                    while self.order.len() > MAX_TEXTURES {
                        if let Some(old) = self.order.pop_front() {
                            self.textures.remove(&old);
                        }
                    }
                }
                None => {
                    self.failed.insert(path);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = ctx;
    }

    pub fn failed(&self, path: &str) -> bool {
        self.failed.contains(path)
    }
}

/// Pictures a second of the preview.
const PREVIEW_FPS: u32 = 8;

/// The silent preview of a moment: an 8 s clip (`video.clip`) shown as successive pictures. The
/// clip is made and decoded on a worker thread; dropping the player tells it to stop.
pub struct Player {
    pub id: u64,
    /// The moment the clip is around.
    pub ms: u64,
    start_ms: u64,
    len_ms: u64,
    frames: Vec<egui::TextureHandle>,
    /// Playback started at this egui time, at this offset (ms into the clip); `None`: paused.
    playing: Option<(f64, f64)>,
    paused_at: f64,
    error: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    rx: Option<std::sync::mpsc::Receiver<Result<Vec<egui::ColorImage>, String>>>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Player {
    /// Pictures decoded, the error if any, playing or not.
    #[cfg(test)]
    pub fn state(&self) -> (usize, Option<&str>, bool) {
        (self.frames.len(), self.error.as_deref(), self.playing.is_some())
    }

    /// Still being made or decoded.
    pub fn loading(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.rx.is_some();
        #[cfg(target_arch = "wasm32")]
        false
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start(app: &LightcraftApp, id: u64, ms: u64) -> Player {
        use std::sync::atomic::Ordering;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut p = Player {
            id,
            ms,
            start_ms: ms,
            len_ms: 0,
            frames: Vec::new(),
            playing: None,
            paused_at: 0.0,
            error: None,
            rx: None,
            cancel: cancel.clone(),
        };
        let job = match lightcraft_engine::video::views::clip_job(&app.session, &json!({"id": id, "ms": ms})) {
            Ok(j) => j,
            Err(e) => {
                p.error = Some(e.to_string());
                return p;
            }
        };
        p.start_ms = job.start_ms;
        p.len_ms = job.len_ms;
        let (tx, rx) = std::sync::mpsc::channel();
        let work = move || {
            let r = (|| {
                job.run()?;
                if cancel.load(Ordering::Relaxed) {
                    return Err("stopped".to_string());
                }
                let jpegs = lightcraft_engine::video::ffmpeg::clip_pictures(&job.out, PREVIEW_FPS, 480, 400)?;
                let mut out = Vec::new();
                for j in jpegs {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("stopped".to_string());
                    }
                    if let Some((w, h, px)) = lightcraft_engine::video::jpeg_rgba(&j, 480)
                        && px.len() == w * h * 4
                    {
                        out.push(egui::ColorImage::from_rgba_unmultiplied([w, h], &px));
                    }
                }
                if out.is_empty() { Err("no picture in the clip".to_string()) } else { Ok(out) }
            })();
            let _ = tx.send(r);
        };
        match std::thread::Builder::new().name("lc-video-preview".into()).spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
        }) {
            Ok(_) => p.rx = Some(rx),
            Err(e) => p.error = Some(e.to_string()),
        }
        p
    }

    #[cfg(target_arch = "wasm32")]
    fn start(_: &LightcraftApp, id: u64, ms: u64) -> Player {
        Player {
            id,
            ms,
            start_ms: ms,
            len_ms: 0,
            frames: Vec::new(),
            playing: None,
            paused_at: 0.0,
            error: Some("no preview in this build".into()),
            cancel: Default::default(),
        }
    }

    fn receive(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(Ok(images)) => {
                    self.frames = images
                        .into_iter()
                        .enumerate()
                        .map(|(i, im)| ctx.load_texture(format!("video-preview-{}-{}-{i}", self.id, self.start_ms), im, egui::TextureOptions::LINEAR))
                        .collect();
                    self.rx = None;
                    self.playing = Some((ctx.input(|i| i.time), 0.0));
                }
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.error = Some("the preview stopped".into());
                    self.rx = None;
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = ctx;
    }

    /// Milliseconds into the clip now.
    fn position(&self, now: f64) -> f64 {
        let len = self.len_ms.max(1) as f64;
        match self.playing {
            Some((t0, from)) => ((now - t0) * 1000.0 + from) % len,
            None => self.paused_at,
        }
    }
}

/// What the view holds between frames.
#[derive(Default)]
pub struct VideosUi {
    /// The preview playing on the sheet, if any.
    pub player: Option<Player>,
    pub section: Section,
    /// The video (or photo) whose moments are shown.
    pub open: Option<u64>,
    /// The moment to bring into view (and outline) on the sheet.
    pub focus_ms: Option<u64>,
    scroll_to_focus: bool,
    pub dirty: bool,
    /// The pictures of the videos being read off the window's thread (the view says "Loading…" meanwhile).
    #[cfg(not(target_arch = "wasm32"))]
    warming: Option<std::sync::mpsc::Receiver<lightcraft_engine::video::ListsLoaded>>,
    list: Option<(u64, Value)>,
    sheet: Option<(u64, u64, Value)>,
    /// Filters of the list.
    pub person: Option<String>,
    pub described: Option<bool>,
    pub status: Option<&'static str>,
    /// Text being typed in a field, by `id:ms:field`.
    edits: HashMap<String, String>,
    /// The last who + what search.
    pub search: Option<Value>,
    pub show_diagnostics: bool,
    pub frames: FrameTextures,
    /// Background runs, asked about now and then: (when, video.status, describe.status, people.status).
    jobs: (f64, Value, Value, Value),
    /// Named people, for the search field's completion (when, names).
    names: (f64, Vec<String>),
    /// After the files of a Video Catcher import are in: the database to read.
    pub after_import: Option<String>,
    /// The Info panel's description of the selected photo or video: (id, catalog revision, moments).
    info: Option<(u64, u64, Value)>,
    /// The pictures of videos where a person is suggested (name, catalog revision, faces), for their page.
    person_faces: Option<(String, u64, Arc<Vec<VideoFace>>)>,
}

/// A face seen in a picture of a video: which video, when, which picture, and where in it.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoFace {
    pub video: u64,
    pub ms: u64,
    pub frame: String,
    pub rect: [f32; 4],
}

/// The pictures of videos where `name` is suggested by the photos' named faces; `None` while the
/// lists of pictures are being read (the page draws "Loading…" and asks again).
pub fn person_faces(app: &mut LightcraftApp, ctx: &egui::Context, name: &str) -> Option<Arc<Vec<VideoFace>>> {
    let rev = app.session.catalog.revision;
    if let Some((n, r, v)) = &app.caches.videos.person_faces
        && n == name
        && *r == rev
    {
        return Some(v.clone());
    }
    if !warm(app, ctx) {
        return None;
    }
    let r = app.session.execute("video.person", &json!({"name": name, "limit": 500})).unwrap_or(Value::Null);
    let rect = |v: &Value| -> Option<[f32; 4]> {
        let a = v.as_array()?;
        Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32, a.get(3)?.as_f64()? as f32])
    };
    let faces: Vec<VideoFace> = r["faces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            Some(VideoFace { video: f["video"].as_u64()?, ms: f["ms"].as_u64()?, frame: f["frame"].as_str()?.to_string(), rect: rect(&f["rect"])? })
        })
        .collect();
    let v = Arc::new(faces);
    app.caches.videos.person_faces = Some((name.to_string(), rev, v.clone()));
    Some(v)
}

impl VideosUi {
    /// Whether the lists of pictures are being read (the headless harness waits for them).
    pub fn is_warming(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.warming.is_some();
        #[cfg(target_arch = "wasm32")]
        false
    }

    /// The catalog revision and annotation edits are what the lists depend on; asked again when either moves.
    fn stale(&self, rev: u64) -> bool {
        self.dirty || self.list.as_ref().is_none_or(|(r, _)| *r != rev)
    }
}

#[cfg(test)]
#[allow(dead_code)] // for the view's UI tests (`tests_videos` of the fork), not ported yet
impl VideosUi {
    /// Put data in the view as if the engine had answered it (tests of what any data draws).
    pub(crate) fn inject(&mut self, rev: u64, list: Option<Value>, sheet: Option<(u64, Value)>, search: Option<Value>) {
        if let Some(l) = list {
            self.list = Some((rev, l));
        }
        if let Some((id, v)) = sheet {
            self.sheet = Some((id, rev, v));
            self.open = Some(id);
        }
        if search.is_some() {
            self.search = search;
        }
        self.dirty = false;
    }
}

/// Plural endings for `n`: English (one, else "s"), French (zero or one, else "s").
fn ends(n: usize) -> (&'static str, &'static str) {
    (if n == 1 { "" } else { "s" }, if n <= 1 { "" } else { "s" })
}

fn n_videos(n: usize) -> String {
    let (en, fr) = ends(n);
    crate::i18n::tr_format!("{n} video{0}{1:.0}", en, fr, n = n)
}

fn n_results(n: usize) -> String {
    let (en, fr) = ends(n);
    crate::i18n::tr_format!("{n} result{0}{1:.0}", en, fr, n = n)
}

pub(crate) fn described_line(n: usize, c: usize) -> String {
    let ((en_n, fr_n), (_, fr_c)) = (ends(n), ends(c));
    crate::i18n::tr_format!("{n} moment{0} described · {c} confirmed{1:.0}{2:.0}", en_n, fr_c, fr_n, n = n, c = c)
}

/// `m:ss` (or `h:mm:ss`).
pub fn timecode(ms: u64) -> String {
    lightcraft_engine::video::views::timecode(ms)
}

/// Run a command; a failure goes to the status line. The lists are asked for again.
fn act(app: &mut LightcraftApp, id: &str, params: Value) -> Option<Value> {
    let r = app.run(id, params);
    app.caches.videos.dirty = true;
    app.caches.videos.person_faces = None;
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            app.ui.status = e;
            None
        }
    }
}

/// Open the sheet of `id` (a video or a photo), at `ms` for a moment of a video.
pub fn open_moment(app: &mut LightcraftApp, id: u64, ms: Option<u64>) {
    let c = &mut app.caches.videos;
    c.open = Some(id);
    c.focus_ms = ms;
    c.scroll_to_focus = ms.is_some();
    c.section = Section::Sheet;
    c.sheet = None;
    app.ui.view = crate::state::ViewMode::Videos;
}

/// Paint the picture at `path` into `r`; with `face` (a box as fractions of the picture), the face
/// cut out of it (a square around it). A spinner while it loads, a dim square if it cannot be read.
pub fn paint_picture(app: &mut LightcraftApp, ui: &egui::Ui, r: Rect, path: &str, face: Option<[f32; 4]>) -> bool {
    let tk = Tokens::get(ui.ctx());
    ui.painter().rect_filled(r, 3.0, tk.canvas);
    if !ui.is_rect_visible(r) {
        return false;
    }
    let Some((tex, [w, h])) = app.caches.videos.frames.get(ui.ctx(), path) else {
        if app.caches.videos.frames.failed(path) {
            ui.painter().text(r.center(), Align2::CENTER_CENTER, "!", tk.semibold(14.0), tk.text_dim);
        } else {
            let a = ui.ctx().input(|i| i.time) as f32 * 4.0;
            ui.painter().circle_filled(r.center() + vec2(a.cos(), a.sin()) * 4.0, 2.0, tk.text_dim);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        return false;
    };
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let uv = match face {
        Some(f) if f.iter().all(|v| v.is_finite()) => {
            let side = ((f[2] - f[0]).abs() * w).max((f[3] - f[1]).abs() * h) * 1.7;
            let side = side.clamp(1.0, w.min(h));
            let (cx, cy) = ((f[0] + f[2]) / 2.0 * w, (f[1] + f[3]) / 2.0 * h);
            let x0 = cx.clamp(side / 2.0, w - side / 2.0) - side / 2.0;
            let y0 = cy.clamp(side / 2.0, h - side / 2.0) - side / 2.0;
            Rect::from_min_max(pos2(x0 / w, y0 / h), pos2((x0 + side) / w, (y0 + side) / h))
        }
        _ => {
            // the whole picture, centred in `r` without stretching
            let k = (r.width() / w).min(r.height() / h);
            let inner = Rect::from_center_size(r.center(), vec2(w * k, h * k));
            ui.painter().image(tex, inner, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            return true;
        }
    };
    ui.painter().image(tex, r, uv, Color32::WHITE);
    true
}

/// A face seen in a video, cut out of the picture it was found in and marked "video · m:ss"; the
/// mark's rectangle is returned (a click on it opens the moment).
pub fn paint_video_face(app: &mut LightcraftApp, ui: &egui::Ui, r: Rect, v: &VideoFace) -> Rect {
    let tk = Tokens::get(ui.ctx());
    paint_picture(app, ui, r, &v.frame, Some(v.rect));
    let text = crate::i18n::tr_format!("video · {t}", t = timecode(v.ms));
    let g = ui.painter().layout_no_wrap(text, tk.semibold(9.5), Color32::WHITE);
    let mark =
        Rect::from_min_size(pos2(r.left() + 2.0, r.bottom() - g.size().y - 4.0), vec2((g.size().x + 6.0).min(r.width() - 4.0), g.size().y + 2.0));
    ui.painter().rect_filled(mark, 2.0, Color32::from_black_alpha(170));
    ui.painter().galley(mark.min + vec2(3.0, 1.0), g, Color32::WHITE);
    mark
}

/// Per frame: the background runs of videos and descriptions, with a Cancel button each, above the
/// app's own task panels.
pub fn jobs(app: &mut LightcraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    let any = [&app.caches.videos.jobs.1["job"], &app.caches.videos.jobs.1["speech"], &app.caches.videos.jobs.2, &app.caches.videos.jobs.3["job"]]
        .iter()
        .any(|j| j["running"] == true);
    if now - app.caches.videos.jobs.0 > if any { 0.5 } else { 3.0 } {
        let v = app.session.execute("video.status", &json!({"jobOnly": true})).unwrap_or(Value::Null);
        let d = app.session.execute("describe.status", &json!({})).unwrap_or(Value::Null);
        let p = if app.caches.videos.jobs.3["job"]["running"] == true {
            app.session.execute("video.facesStatus", &json!({})).unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let was = any;
        app.caches.videos.jobs = (now, v, d, p);
        let still =
            [&app.caches.videos.jobs.1["job"], &app.caches.videos.jobs.1["speech"], &app.caches.videos.jobs.2, &app.caches.videos.jobs.3["job"]]
                .iter()
                .any(|j| j["running"] == true);
        if was && !still {
            app.caches.videos.dirty = true;
            app.caches.videos.person_faces = None;
        }
    }
    // a Video Catcher import whose files are in: now its pictures and moments
    if app.import.is_none()
        && let Some(db) = app.caches.videos.after_import.take()
    {
        match app.run("video.importVideoCatcher", json!({"db": db})) {
            Ok(r) => app.toast(
                ctx,
                crate::i18n::tr_format!(
                    "Video Catcher: {m} videos, {a} descriptions added",
                    m = r["matched"].as_u64().unwrap_or(0),
                    a = r["annotationsAdded"].as_u64().unwrap_or(0)
                ),
            ),
            Err(e) => app.toast_error(ctx, e),
        }
        app.caches.videos.dirty = true;
    }
    let mut slot = app.tasks.labels().len();
    let rows = [
        ("videoFrames", "Cutting pictures from videos", app.caches.videos.jobs.1["job"].clone(), "video.cancel"),
        ("videoSpeech", "Listening to videos", app.caches.videos.jobs.1["speech"].clone(), "video.cancel"),
        ("describeSuggest", "Proposing descriptions", app.caches.videos.jobs.2.clone(), "describe.cancel"),
        ("videoFaces", "Finding faces", app.caches.videos.jobs.3["job"].clone(), "video.facesCancel"),
    ];
    for (key, label, j, cancel) in rows {
        if j["running"] != true {
            continue;
        }
        let (done, total) = (j["done"].as_u64().unwrap_or(0) as usize, j["total"].as_u64().unwrap_or(0) as usize);
        if crate::tasks::progress_window(ctx, key, slot, label, (done, total), "", j["cancelled"] == true) {
            let _ = app.run(cancel, json!({}));
        }
        slot += 1;
    }
}

/// The view.
/// Make sure the lists of pictures are remembered before the view asks the engine for anything: they are read on a
/// worker (355 videos on a network or USB volume took 14 s, up to 134 s under load, on the window's thread).
/// `false` while they load: the view then draws a "Loading…" page.
fn warmed(app: &mut LightcraftApp, ui: &mut egui::Ui) -> bool {
    if warm(app, ui.ctx()) {
        return true;
    }
    let t = Tokens::get(ui.ctx());
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.spinner();
        ui.add_space(8.0);
        let r = ui.label(egui::RichText::new(tr("Loading the videos…")).font(t.font(14.0)).color(t.text_dim));
        register(ui, "videos:loading", r.rect);
    });
    false
}

/// Start the reading of the lists of pictures on a worker (or collect it); `true` once they are remembered. Whatever
/// shows the faces of videos (the Videos view, a person's page) asks this before asking the engine, and draws
/// "Loading…" while it is `false`.
pub fn warm(app: &mut LightcraftApp, ctx: &egui::Context) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if app.session.video_frames_warm() {
            app.caches.videos.warming = None;
            return true;
        }
        if let Some(rx) = &app.caches.videos.warming {
            match rx.try_recv() {
                Ok(loaded) => {
                    app.caches.videos.warming = None;
                    app.session.video_frames_apply(loaded);
                    app.caches.videos.dirty = true;
                    return app.session.video_frames_warm();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => app.caches.videos.warming = None,
            }
        } else if let Some(job) = app.session.video_frames_job() {
            let (tx, rx) = std::sync::mpsc::channel();
            let repaint = ctx.clone();
            let started = std::thread::Builder::new().name("lc-video-lists".into()).spawn(move || {
                let _ = tx.send(job.run());
                repaint.request_repaint();
            });
            if started.is_ok() {
                app.caches.videos.warming = Some(rx);
            } else {
                return true;
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
        false
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, ctx);
        true
    }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if !warmed(app, ui) {
        return;
    }
    match app.caches.videos.section {
        Section::Sheet if app.caches.videos.open.is_some() => sheet(app, ui),
        Section::Search if app.caches.videos.search.is_some() => results(app, ui),
        _ => list(app, ui),
    }
}

fn title_row(app: &mut LightcraftApp, ui: &mut egui::Ui, back: bool, title: &str, sub: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut went_back = false;
    ui.horizontal(|ui| {
        if back && icon_button(ui, "videosBack", Icon::Back, vec2(28.0, 28.0), false, true, "Back to the videos").clicked() {
            went_back = true;
        }
        ui.label(egui::RichText::new(title).font(t.semibold(18.0)).color(t.text));
        if !sub.is_empty() {
            ui.label(egui::RichText::new(sub).font(t.font(13.0)).color(t.text_dim));
        }
    });
    if back && ui.input(|i| i.key_pressed(egui::Key::Escape)) && !ui.ctx().egui_wants_keyboard_input() {
        went_back = true;
    }
    if went_back {
        app.caches.videos.section = Section::List;
    }
    went_back
}

// ------------------------------------------------------------------------------------ the list

fn list(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let rev = app.session.catalog.revision;
    if app.caches.videos.stale(rev) {
        let c = &app.caches.videos;
        let mut p = json!({});
        if let Some(n) = &c.person {
            p["person"] = json!(n);
        }
        if let Some(d) = c.described {
            p["described"] = json!(d);
        }
        if let Some(s) = c.status {
            p["status"] = json!(s);
        }
        let mut v = app.session.execute("video.list", &p).unwrap_or(Value::Null);
        v["unreadable"] = app.session.execute("video.unreadable", &json!({})).map(|u| u["videos"].clone()).unwrap_or(Value::Null);
        app.caches.videos.list = Some((rev, v));
        app.caches.videos.dirty = false;
    }
    let v = app.caches.videos.list.as_ref().map(|(_, v)| v.clone()).unwrap_or(Value::Null);
    let rows = v["videos"].as_array().cloned().unwrap_or_default();
    let t = Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().id_salt("videos-list").auto_shrink(false).show(ui, |ui| {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.vertical(|ui| {
                let n = rows.len();
                title_row(app, ui, false, tr("Videos"), &n_videos(n));
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if text_button(ui, "videosImport", "Import videos…", false)
                        .on_hover_text(tr("Adds the videos of a folder (and its photos) to the library"))
                        .clicked()
                    {
                        let _ = app.run("videos.import", json!({}));
                    }
                    if text_button(ui, "videosVideoCatcher", "Resume the Video Catcher import", false)
                        .on_hover_text(tr("Brings in the videos, pictures and descriptions Video Catcher already made"))
                        .clicked()
                    {
                        let _ = app.run("videos.importVideoCatcher", json!({}));
                    }
                    if n > 0 {
                        let ids: Vec<u64> = rows.iter().filter_map(|r| r["id"].as_u64()).collect();
                        if text_button(ui, "videosFrames", "Cut pictures", false)
                            .on_hover_text(tr("Cuts a few pictures out of each video, to find faces and describe moments"))
                            .clicked()
                        {
                            act(app, "video.frames", json!({"ids": ids, "background": true}));
                        }
                        if text_button(ui, "videosFaces", "Find faces", false)
                            .on_hover_text(tr("Looks for faces in the pictures of these videos"))
                            .clicked()
                        {
                            act(app, "video.faces", json!({"ids": ids, "background": true}));
                        }
                        if text_button(ui, "videosScenes", "Cut pictures at shots", false)
                            .on_hover_text(tr(
                                "Reads each video to find where the shot changes, then cuts a picture in every shot (more in a long one)",
                            ))
                            .clicked()
                        {
                            act(app, "video.frames", json!({"ids": ids, "plan": "scenes", "background": true}));
                        }
                        if text_button(ui, "videosRetry", "Retry failed pictures", false)
                            .on_hover_text(tr("Cuts again the pictures that could not be cut, and the ones just beside them"))
                            .clicked()
                        {
                            act(app, "video.frames", json!({"ids": ids, "retryFailed": true, "background": true}));
                        }
                        if text_button(ui, "videosSpeech", "Transcribe speech", false)
                            .on_hover_text(tr("Listens to the videos (Whisper, one at a time) so that what is said can be searched"))
                            .clicked()
                        {
                            act(app, "video.transcribe", json!({"ids": ids, "background": true}));
                        }
                    }
                });
                ui.add_space(8.0);
                filters(app, ui, &v);
                ui.add_space(10.0);
                unreadable(app, ui, &v["unreadable"]);
                if rows.is_empty() {
                    let msg = if app.caches.videos.person.is_some() || app.caches.videos.described.is_some() || app.caches.videos.status.is_some() {
                        "No video matches these filters"
                    } else {
                        "No video in the library yet: import a folder of videos"
                    };
                    ui.label(egui::RichText::new(tr(msg)).color(t.text_dim));
                    return;
                }
                let per_row = ((ui.available_width() + 12.0) / (VCARD + 12.0)).floor().max(1.0) as usize;
                for chunk in rows.chunks(per_row) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        for r in chunk {
                            video_card(app, ui, r);
                        }
                    });
                    ui.add_space(12.0);
                }
            });
        });
    });
}

/// The videos imports could not read: their cause in plain words and the way to their folder.
fn unreadable(app: &mut LightcraftApp, ui: &mut egui::Ui, list: &Value) {
    let rows = list.as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let id = egui::Id::unique("videos-unreadable-open");
    let open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let label = crate::i18n::tr_format!("Unreadable ({n})", n = rows.len());
    if text_button(ui, "videosUnreadable", &label, open).on_hover_text(tr("Videos an import could not read")).clicked() {
        ui.data_mut(|d| d.insert_temp(id, !open));
    }
    if !open {
        ui.add_space(8.0);
        return;
    }
    for (i, r) in rows.iter().enumerate() {
        let cause = match r["cause"].as_str().unwrap_or("") {
            "empty" => tr("Empty file (0 bytes)"),
            "noIndex" => tr("Cut short: the video's index (moov) is missing; it cannot be rebuilt without a sound file from the same camera"),
            "noPicture" => tr("No picture in this file"),
            "missing" => tr("The file is no longer there"),
            _ => tr("The file cannot be read"),
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(r["name"].as_str().unwrap_or("")).font(t.semibold(12.0)).color(t.text));
            ui.label(egui::RichText::new(cause).font(t.font(11.5)).color(t.caution)).on_hover_text(r["error"].as_str().unwrap_or(""));
            if let Some(folder) = r["folder"].as_str()
                && text_button(ui, &format!("unreadableReveal{i}"), "Open the folder", false).clicked()
            {
                let res = match app.services.reveal.as_mut() {
                    Some(f) => f(r["path"].as_str().unwrap_or(folder)),
                    None => Err(tr("No file manager here").to_string()),
                };
                if let Err(e) = res {
                    app.ui.status = e;
                }
            }
        });
    }
    ui.add_space(8.0);
}

fn filters(app: &mut LightcraftApp, ui: &mut egui::Ui, v: &Value) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(tr("Person")).color(t.text_label));
        let current = app.caches.videos.person.clone().unwrap_or_else(|| tr("Everyone").to_string());
        let named = crate::widgets::combo_box("videosPerson").selected_text(current).show_ui(ui, |ui| {
            if ui.selectable_label(app.caches.videos.person.is_none(), tr("Everyone")).clicked() {
                app.caches.videos.person = None;
                app.caches.videos.dirty = true;
            }
            for n in v["people"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                if ui.selectable_label(app.caches.videos.person.as_deref() == Some(n), n).clicked() {
                    app.caches.videos.person = Some(n.to_string());
                    app.caches.videos.dirty = true;
                }
            }
        });
        crate::widgets::name_combo(&named.response, tr("Person"));
        ui.add_space(12.0);
        let d = match app.caches.videos.described {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        };
        let pick = ui
            .allocate_ui(vec2(330.0, 24.0), |ui| {
                crate::widgets::segmented(ui, "videosDescribed", &[("All", "all"), ("Described", "yes"), ("Not described", "no")], Some(d), 3)
            })
            .inner;
        if let Some(i) = pick {
            app.caches.videos.described = [None, Some(true), Some(false)].get(i).copied().flatten();
            app.caches.videos.dirty = true;
        }
        ui.add_space(12.0);
        let s = match app.caches.videos.status {
            Some("confirmed") => 1,
            Some(_) => 2,
            None => 0,
        };
        let pick = ui
            .allocate_ui(vec2(330.0, 24.0), |ui| {
                crate::widgets::segmented(ui, "videosStatus", &[("All", "all"), ("Confirmed", "confirmed"), ("To confirm", "toConfirm")], Some(s), 3)
            })
            .inner;
        if let Some(i) = pick {
            app.caches.videos.status = [None, Some("confirmed"), Some("toConfirm")].get(i).copied().flatten();
            app.caches.videos.dirty = true;
        }
    });
}

/// A small rounded mark: a person's name, a state.
fn chip(ui: &mut egui::Ui, id: &str, text: &str, on: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let g = ui.painter().layout_no_wrap(text.to_string(), t.font(11.0), if on { t.text } else { t.text_label });
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 20.0), Sense::click());
    register(ui, format!("chip:{id}"), r);
    let fill = if resp.hovered() {
        t.hover
    } else if on {
        t.pressed
    } else {
        t.button
    };
    ui.painter().rect(r, 10.0, fill, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
    ui.painter().galley(pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0), g, t.text);
    resp
}

/// Where a video's analysis stands: pictures cut, faces looked for, described.
fn states(ui: &mut egui::Ui, r: &Value) {
    let t = Tokens::get(ui.ctx());
    let frames = r["frames"].as_u64().unwrap_or(0);
    let looked = r["facesLooked"].as_u64().unwrap_or(0);
    let moments = r["moments"].as_u64().unwrap_or(0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for (done, label) in [(frames > 0, "Pictures"), (frames > 0 && looked >= frames, "Faces"), (moments > 0, "Described")] {
            let c = if done { t.accent } else { t.text_disabled };
            // a dot (filled when done), then the step
            let (dot, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
            if done {
                ui.painter().circle_filled(dot.center(), 3.5, c);
            } else {
                ui.painter().circle_stroke(dot.center(), 3.0, Stroke::new(1.0, c));
            }
            ui.label(egui::RichText::new(tr(label)).font(t.font(10.5)).color(c));
        }
    });
}

fn video_card(app: &mut LightcraftApp, ui: &mut egui::Ui, r: &Value) {
    let t = Tokens::get(ui.ctx());
    let Some(id) = r["id"].as_u64() else { return };
    ui.vertical(|ui| {
        ui.set_width(VCARD);
        let (pr, resp) = ui.allocate_exact_size(vec2(VCARD, VCARD * 9.0 / 16.0), Sense::click());
        register(ui, format!("videoCard:{id}"), pr);
        let shown = match r["poster"].as_str() {
            Some(p) => paint_picture(app, ui, pr, p, None),
            None => false,
        };
        if !shown {
            // no pictures cut yet: the grid's poster
            let pid = lightcraft_catalog::PhotoId(id);
            super::grid::request_thumb(app, pid, 256, 8);
            if let Some(tex) = app.renderer.thumb(pid) {
                ui.painter().image(tex.tex.id(), pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
        if let Some(d) = r["duration"].as_f64().filter(|d| d.is_finite() && *d >= 0.0) {
            let text = timecode((d * 1000.0) as u64);
            let g = ui.painter().layout_no_wrap(text, t.semibold(10.5), Color32::WHITE);
            let br = Rect::from_min_size(pr.right_bottom() - vec2(g.size().x + 10.0, g.size().y + 8.0), vec2(g.size().x + 6.0, g.size().y + 4.0));
            ui.painter().rect_filled(br, 3.0, Color32::from_black_alpha(170));
            ui.painter().galley(br.min + vec2(3.0, 2.0), g, Color32::WHITE);
        }
        if resp.hovered() {
            ui.painter().rect_stroke(pr, 3.0, Stroke::new(1.5, t.accent), StrokeKind::Outside);
        }
        if resp.clicked() {
            open_moment(app, id, None);
        }
        let name = r["name"].as_str().unwrap_or_default();
        let g = crate::widgets::one_line(ui.painter(), name, t.semibold(12.0), t.text, VCARD);
        let (nr, _) = ui.allocate_exact_size(vec2(VCARD, 18.0), Sense::hover());
        ui.painter().galley(nr.left_center() - vec2(0.0, g.size().y / 2.0), g, t.text);
        let people: Vec<&str> = r["people"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for n in people.iter().take(4) {
                if chip(ui, &format!("videoPerson:{id}:{n}"), n, false).on_hover_text(tr("Shows this person's photos and videos")).clicked() {
                    let _ = app.run("view.person", json!({"name": n}));
                }
            }
            if people.len() > 4 {
                ui.label(egui::RichText::new(format!("+{}", people.len() - 4)).color(t.text_dim));
            }
        });
        states(ui, r);
        let (m, c) = (r["moments"].as_u64().unwrap_or(0), r["confirmed"].as_u64().unwrap_or(0));
        if m > 0 {
            ui.label(egui::RichText::new(described_line(m as usize, c as usize)).font(t.font(10.5)).color(t.text_dim));
        }
    });
}

// ------------------------------------------------------------------------------------ the sheet

fn sheet(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let Some(id) = app.caches.videos.open else { return };
    let rev = app.session.catalog.revision;
    let fresh = app.caches.videos.sheet.as_ref().is_some_and(|(i, r, _)| *i == id && *r == rev) && !app.caches.videos.dirty;
    if !fresh {
        let v = app.session.execute("video.moments", &json!({"id": id})).unwrap_or(Value::Null);
        app.caches.videos.sheet = Some((id, rev, v));
        app.caches.videos.dirty = false;
    }
    let v = app.caches.videos.sheet.as_ref().map(|(_, _, v)| v.clone()).unwrap_or(Value::Null);
    let moments = v["moments"].as_array().cloned().unwrap_or_default();
    let video = v["video"] == true;
    let t = Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().id_salt("videos-sheet").auto_shrink(false).show(ui, |ui| {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.vertical(|ui| {
                let name = v["name"].as_str().unwrap_or_default().to_string();
                let dur = v["duration"].as_f64().filter(|d| d.is_finite() && *d >= 0.0).map(|d| timecode((d * 1000.0) as u64)).unwrap_or_default();
                if title_row(app, ui, true, &name, &dur) {
                    return;
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if video && text_button(ui, "sheetPlay", "Play", false).on_hover_text(tr("Opens the video in your video player")).clicked() {
                        act(app, "video.open", json!({"id": id}));
                    }
                    if text_button(ui, "sheetSuggest", "Propose descriptions", false)
                        .on_hover_text(tr("Proposes a scene and an action for each moment; nothing is confirmed without you"))
                        .clicked()
                    {
                        act(app, "describe.suggest", json!({"ids": [id], "background": true}));
                    }
                    if video {
                        if text_button(ui, "sheetFrames", "Cut pictures", false).clicked() {
                            act(app, "video.frames", json!({"ids": [id], "background": true}));
                        }
                        if text_button(ui, "sheetFaces", "Find faces", false).clicked() {
                            act(app, "video.faces", json!({"ids": [id], "background": true}));
                        }
                    }
                });
                ui.add_space(10.0);
                preview(app, ui, id);
                if moments.is_empty() {
                    ui.label(
                        egui::RichText::new(tr(if video { "No moment yet: cut pictures from this video first" } else { "Nothing to show" }))
                            .color(t.text_dim),
                    );
                    return;
                }
                let per_row = ((ui.available_width() + 14.0) / (CARD + 14.0)).floor().max(1.0) as usize;
                for chunk in moments.chunks(per_row) {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        for m in chunk {
                            ui.vertical(|ui| {
                                ui.set_width(CARD);
                                moment_card(app, ui, id, m, CARD, video);
                            });
                        }
                    });
                    ui.add_space(16.0);
                }
            });
        });
    });
}

/// The silent preview at the top of the sheet: the pictures of an 8 s clip, a position bar, -5 s /
/// +5 s, play / pause, the system player, close.
fn preview(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u64) {
    let t = Tokens::get(ui.ctx());
    if app.caches.videos.player.as_ref().is_some_and(|p| p.id != id) {
        app.caches.videos.player = None;
    }
    let Some(mut p) = app.caches.videos.player.take() else { return };
    p.receive(ui.ctx());
    let now = ui.input(|i| i.time);
    let (w, h) = (480.0_f32, 270.0_f32);
    let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    register(ui, "videoPreview", r);
    ui.painter().rect_filled(r, 4.0, Color32::BLACK);
    let pos = p.position(now);
    if let Some(e) = &p.error {
        ui.painter().text(r.center(), Align2::CENTER_CENTER, format!("{} {e}", tr("No preview:")), t.font(12.0), t.text_dim);
    } else if p.frames.is_empty() {
        ui.painter().text(r.center(), Align2::CENTER_CENTER, tr("Preparing the preview…"), t.font(12.0), t.text_dim);
    } else {
        let n = p.frames.len();
        let i = ((pos / p.len_ms.max(1) as f64) * n as f64).floor().clamp(0.0, (n - 1) as f64) as usize;
        if let Some(tex) = p.frames.get(i) {
            let [tw, th] = tex.size();
            let k = (w / tw.max(1) as f32).min(h / th.max(1) as f32);
            let inner = Rect::from_center_size(r.center(), vec2(tw as f32 * k, th as f32 * k));
            ui.painter().image(tex.id(), inner, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        if p.playing.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(1000 / u64::from(PREVIEW_FPS)));
        }
    }
    if resp.clicked() && !p.frames.is_empty() {
        p.playing = match p.playing {
            Some(_) => {
                p.paused_at = pos;
                None
            }
            None => Some((now, p.paused_at)),
        };
    }
    // the position bar: drag to move inside the clip
    let (bar, bresp) = ui.allocate_exact_size(vec2(w, 10.0), Sense::click_and_drag());
    register(ui, "videoPreviewBar", bar);
    ui.painter().rect_filled(bar, 3.0, t.track);
    let frac = (pos / p.len_ms.max(1) as f64) as f32;
    ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac.clamp(0.0, 1.0), bar.height())), 3.0, t.accent);
    if let Some(x) = bresp.interact_pointer_pos().filter(|_| bresp.dragged() || bresp.clicked()) {
        let f = ((x.x - bar.left()) / bar.width()).clamp(0.0, 1.0) as f64;
        p.paused_at = f * p.len_ms as f64;
        if p.playing.is_some() {
            p.playing = Some((now, p.paused_at));
        }
    }
    let at = p.start_ms + pos as u64;
    let mut next: Option<u64> = None;
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(timecode(at)).font(t.semibold(12.0)).color(t.text));
        ui.label(egui::RichText::new(tr("Preview without sound")).font(t.font(11.0)).color(t.text_dim));
        if text_button(ui, "previewBack", "-5 s", false).clicked() {
            next = Some(at.saturating_sub(5000));
        }
        let playing = p.playing.is_some();
        if text_button(ui, "previewPlay", if playing { "Pause" } else { "Play" }, false).clicked() && !p.frames.is_empty() {
            p.playing = if playing {
                p.paused_at = pos;
                None
            } else {
                Some((now, p.paused_at))
            };
        }
        if text_button(ui, "previewForward", "+5 s", false).clicked() {
            next = Some(at + 5000);
        }
        if text_button(ui, "previewVlc", "Play in VLC", false).on_hover_text(tr("Opens the video in your video player at this time")).clicked() {
            act(app, "video.open", json!({"id": id, "ms": at}));
        }
        if text_button(ui, "previewClose", "Close", false).clicked() {
            close = true;
        }
    });
    ui.add_space(10.0);
    app.caches.videos.player = if close {
        None
    } else if let Some(ms) = next {
        Some(Player::start(app, id, ms))
    } else {
        Some(p)
    };
}

/// The editor of one moment's description (also the photo's, in the Info panel): the picture (a
/// video's), the time, who is there, then scene, action and description — Enter saves what is
/// typed, a proposal is one click away, Confirm is the explicit "this is right".
pub fn moment_card(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u64, m: &Value, width: f32, video: bool) {
    let t = Tokens::get(ui.ctx());
    let ms = m["ms"].as_u64();
    let key = format!("{id}:{}", ms.map_or_else(|| "-".to_string(), |x| x.to_string()));
    let focused = video && ms.is_some() && app.caches.videos.focus_ms == ms;
    if let Some(frame) = m["frame"].as_str() {
        let (r, resp) = ui.allocate_exact_size(vec2(width, width * 9.0 / 16.0), Sense::click());
        register(ui, format!("moment:{key}"), r);
        paint_picture(app, ui, r, frame, None);
        // the faces, outlined thinly
        for f in m["faces"].as_array().into_iter().flatten() {
            let b: Vec<f32> = f["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).map(|x| x as f32).collect();
            if let [x0, y0, x1, y1] = b[..] {
                let fr = Rect::from_min_max(
                    pos2(r.left() + x0 * r.width(), r.top() + y0 * r.height()),
                    pos2(r.left() + x1 * r.width(), r.top() + y1 * r.height()),
                );
                ui.painter().rect_stroke(fr, 2.0, Stroke::new(1.0, Color32::from_white_alpha(170)), StrokeKind::Outside);
            }
        }
        if focused {
            ui.painter().rect_stroke(r, 3.0, Stroke::new(2.5, t.accent), StrokeKind::Outside);
            if std::mem::take(&mut app.caches.videos.scroll_to_focus) {
                ui.scroll_to_rect(r, Some(egui::Align::Center));
            }
        }
        if resp.double_clicked() {
            act(app, "video.open", json!({"id": id, "ms": ms.unwrap_or(0)}));
        }
    }
    let a = &m["annotation"];
    ui.horizontal(|ui| {
        if let Some(ms) = ms {
            ui.label(egui::RichText::new(timecode(ms)).font(t.semibold(12.0)).color(t.text));
            if video
                && text_button(ui, &format!("preview:{key}"), "Preview", false)
                    .on_hover_text(tr("Plays 8 seconds around this moment here, without sound"))
                    .clicked()
            {
                app.caches.videos.player = Some(Player::start(app, id, ms));
            }
            if video && text_button(ui, &format!("play:{key}"), &crate::i18n::tr_format!("Play at {t}", t = timecode(ms)), false).clicked() {
                act(app, "video.open", json!({"id": id, "ms": ms}));
            }
        }
        if !a.is_null() {
            let confirmed = a["status"] == "confirmed";
            let (text, c) = if confirmed { (tr("Confirmed"), t.accent) } else { (tr("Proposed"), t.caution) };
            ui.label(egui::RichText::new(text).font(t.semibold(10.5)).color(c));
        }
    });
    // who is there
    let faces = m["faces"].as_array().cloned().unwrap_or_default();
    if !faces.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            let mut unnamed = 0;
            for f in &faces {
                match f["name"].as_str() {
                    Some(n) => {
                        if chip(ui, &format!("who:{key}:{n}"), n, true).on_hover_text(tr("Shows this person's photos and videos")).clicked() {
                            let _ = app.run("view.person", json!({"name": n}));
                        }
                    }
                    None => unnamed += 1,
                }
            }
            if unnamed > 0 && chip(ui, &format!("whoUnnamed:{key}"), tr("Faces to name"), false).clicked() {
                let _ = app.run("view.people", json!({}));
            }
        });
    }
    let proposals = a["proposals"].as_array().cloned().unwrap_or_default();
    for (field, label, lines) in [("scene", "Scene", 1), ("action", "Action", 1), ("description", "Description", 2)] {
        let value = a[field].as_str().unwrap_or_default().to_string();
        let ek = format!("{key}:{field}");
        let name = ui.label(egui::RichText::new(tr(label)).font(t.font(10.5)).color(t.text_dim));
        let mut text = app.caches.videos.edits.get(&ek).cloned().unwrap_or_else(|| value.clone());
        let edit = if lines > 1 {
            egui::TextEdit::multiline(&mut text).desired_rows(lines).desired_width(width - 12.0).id(egui::Id::unique(("moment-field", &ek)))
        } else {
            egui::TextEdit::singleline(&mut text).desired_width(width - 12.0).id(egui::Id::unique(("moment-field", &ek)))
        };
        let resp = ui.add(edit).labelled_by(name.id);
        register(ui, format!("field:{ek}"), resp.rect);
        if resp.changed() {
            app.caches.videos.edits.insert(ek.clone(), text.clone());
        }
        // Enter (or leaving the field) saves what was typed; Shift+Enter makes a new line in a description
        let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        if (enter || resp.lost_focus()) && app.caches.videos.edits.contains_key(&ek) {
            let typed = app.caches.videos.edits.remove(&ek).unwrap_or_default();
            if typed.trim() != value.trim() {
                let mut p = json!({"id": id, field: typed.trim_end_matches('\n')});
                if let Some(ms) = ms {
                    p["ms"] = json!(ms);
                }
                act(app, "describe.set", p);
            }
            if enter {
                resp.surrender_focus();
            }
        }
        // the programs' proposals that differ from the value: one click takes one
        let offers: Vec<String> =
            proposals.iter().filter_map(|p| p[field].as_str()).filter(|s| !s.trim().is_empty() && s.trim() != value.trim()).map(str::to_string).fold(
                Vec::new(),
                |mut v, s| {
                    if !v.contains(&s) {
                        v.push(s);
                    }
                    v
                },
            );
        if !offers.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for (i, o) in offers.iter().take(3).enumerate() {
                    let short: String = if o.chars().count() > 40 { format!("{}…", o.chars().take(39).collect::<String>()) } else { o.clone() };
                    if chip(ui, &format!("offer:{ek}:{i}"), &format!("+ {short}"), false).on_hover_text(tr("Use this proposal")).clicked() {
                        let mut p = json!({"id": id, field: o});
                        if let Some(ms) = ms {
                            p["ms"] = json!(ms);
                        }
                        act(app, "describe.set", p);
                    }
                }
            });
        }
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let can = !a.is_null() && a["status"] != "confirmed";
        let mut p = json!({"id": id});
        if let Some(ms) = ms {
            p["ms"] = json!(ms);
        }
        if can && text_button(ui, &format!("confirm:{key}"), "Confirm", false).on_hover_text(tr("Says this description is right")).clicked() {
            act(app, "describe.confirm", p.clone());
        }
        let mine = a["status"] == "confirmed" || a["from"].as_array().is_some_and(|f| f.iter().any(|x| x == "manual"));
        if mine
            && text_button(ui, &format!("clear:{key}"), "Clear mine", false)
                .on_hover_text(tr("Takes back what you wrote; the proposals show again"))
                .clicked()
        {
            act(app, "describe.delete", p);
        }
    });
}

/// The Info panel's "Description": for a photo, its editor; for a video, how many moments are
/// described and the way to its sheet.
pub fn info_section(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u64, width: f32, title: bool) {
    let width = (width - 8.0).max(80.0);
    let t = Tokens::get(ui.ctx());
    let rev = app.session.catalog.revision;
    let fresh = !app.caches.videos.dirty && app.caches.videos.info.as_ref().is_some_and(|(i, r, _)| *i == id && *r == rev);
    if !fresh {
        let v = app.session.execute("video.moments", &json!({"id": id})).unwrap_or(Value::Null);
        app.caches.videos.info = Some((id, rev, v));
    }
    let v = app.caches.videos.info.as_ref().map(|(_, _, v)| v.clone()).unwrap_or(Value::Null);
    if title {
        ui.label(egui::RichText::new(tr("Description")).font(t.semibold(12.5)).color(t.text));
        ui.add_space(4.0);
    }
    if v["video"] == true {
        let described = v["moments"].as_array().into_iter().flatten().filter(|m| !m["annotation"].is_null()).count();
        ui.label(
            egui::RichText::new(described_line(
                described,
                v["moments"].as_array().into_iter().flatten().filter(|m| m["annotation"]["status"] == "confirmed").count(),
            ))
            .color(t.text_dim),
        );
        if text_button(ui, "infoOpenVideo", "Open the moments", false).clicked() {
            open_moment(app, id, None);
        }
        return;
    }
    if let Some(m) = v["moments"].as_array().and_then(|a| a.first()).cloned() {
        moment_card(app, ui, id, &m, width, false);
    }
    if text_button(ui, "infoSuggest", "Propose descriptions", false)
        .on_hover_text(tr("Proposes a scene and an action for the selected photos; nothing is confirmed without you"))
        .clicked()
    {
        let ids: Vec<u64> = if app.session.selection.ids.is_empty() { vec![id] } else { app.session.selection.ids.iter().map(|i| i.0).collect() };
        act(app, "describe.suggest", json!({"ids": ids, "background": true}));
    }
}

// ------------------------------------------------------------------------------------ search

/// The search field asked for `query` (Enter): `semantic: …` searches by meaning (photos), any
/// other text searches who + what over photos and videos; the results open in this view.
pub fn search_from_field(app: &mut LightcraftApp, query: &str) {
    let q = query.trim();
    if q.is_empty() {
        return;
    }
    if let Some(rest) = q.strip_prefix("semantic:").map(str::trim).filter(|r| !r.is_empty()) {
        match app.run("search.semantic", json!({"query": rest, "filter": true})) {
            Ok(r) => {
                let n = r["photos"].as_array().map_or(0, Vec::len);
                app.ui.status = n_results(n);
            }
            Err(e) => app.ui.status = e,
        }
        return;
    }
    match app.run("describe.search", json!({"query": q, "limit": 120})) {
        Ok(r) => {
            app.caches.videos.search = Some(r);
            app.caches.videos.section = Section::Search;
            app.caches.videos.show_diagnostics = false;
            app.ui.view = crate::state::ViewMode::Videos;
        }
        Err(e) => app.ui.status = e,
    }
}

/// Named people whose name continues the last word of `text` (for the search field).
pub fn complete_names(app: &mut LightcraftApp, now: f64, text: &str) -> Vec<String> {
    if now - app.caches.videos.names.0 > 5.0 {
        let v = Value::Null;
        let _ = v;
        let names: Vec<String> = app.session.catalog.people().into_iter().map(|p| p.name).collect();
        app.caches.videos.names = (now, names);
    }
    let last = text.rsplit(' ').next().unwrap_or("").to_lowercase();
    if last.chars().count() < 2 {
        return Vec::new();
    }
    app.caches.videos.names.1.iter().filter(|n| n.to_lowercase().starts_with(&last) && n.to_lowercase() != last).take(5).cloned().collect()
}

fn results(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let Some(r) = app.caches.videos.search.clone() else { return };
    let t = Tokens::get(ui.ctx());
    let rows = r["results"].as_array().cloned().unwrap_or_default();
    egui::ScrollArea::vertical().id_salt("videos-results").auto_shrink(false).show(ui, |ui| {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.vertical(|ui| {
                let q = r["query"].as_str().unwrap_or_default();
                let total = r["total"].as_u64().unwrap_or(0);
                if title_row(app, ui, true, &format!("« {q} »"), &n_results(total as usize)) {
                    return;
                }
                // what the search understood, in plain words
                let people: Vec<&str> = r["people"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let words: Vec<&str> = r["words"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                ui.horizontal_wrapped(|ui| {
                    if !people.is_empty() {
                        ui.label(egui::RichText::new(tr("Who")).color(t.text_dim));
                        for p in &people {
                            chip(ui, &format!("searchWho:{p}"), p, true);
                        }
                    }
                    if !words.is_empty() {
                        ui.label(egui::RichText::new(tr("What")).color(t.text_dim));
                        ui.label(egui::RichText::new(words.join(" ")).color(t.text));
                    }
                });
                ui.add_space(10.0);
                if rows.is_empty() {
                    ui.label(egui::RichText::new(tr("Nothing found")).color(t.text_dim));
                }
                result_grid(app, ui, &rows, "res");
                ui.add_space(14.0);
                // apart, never mixed in: the people are there but the words were not found
                let d = &r["diagnostics"]["personPresentWhatNotDetected"];
                let n = d["total"].as_u64().unwrap_or(0);
                if n > 0 {
                    let label = crate::i18n::tr_format!("Person present, action not detected ({n})", n = n);
                    let open = app.caches.videos.show_diagnostics;
                    if text_button(ui, "searchDiagnostics", &label, open).clicked() {
                        app.caches.videos.show_diagnostics = !open;
                    }
                    if open {
                        ui.label(
                            egui::RichText::new(tr(
                                "Not detected is not absent: a description may not say it, or a face may be too small or not confirmed",
                            ))
                            .font(t.font(11.0))
                            .color(t.text_dim),
                        );
                        let ex = d["examples"].as_array().cloned().unwrap_or_default();
                        result_grid(app, ui, &ex, "diag");
                    }
                }
                let w = &r["diagnostics"]["whatFoundPeopleNotRecognized"];
                let n = w["total"].as_u64().unwrap_or(0);
                if n > 0 {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(crate::i18n::tr_format!("Action found, person not recognized ({n})", n = n))
                            .font(t.font(11.5))
                            .color(t.text_dim),
                    );
                }
            });
        });
    });
}

fn result_grid(app: &mut LightcraftApp, ui: &mut egui::Ui, rows: &[Value], salt: &str) {
    let t = Tokens::get(ui.ctx());
    let w = 200.0;
    let per_row = ((ui.available_width() + 12.0) / (w + 12.0)).floor().max(1.0) as usize;
    for chunk in rows.chunks(per_row) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            for x in chunk {
                let Some(id) = x["id"].as_u64() else { continue };
                let ms = x["ms"].as_u64();
                ui.vertical(|ui| {
                    ui.set_width(w);
                    let (r, resp) = ui.allocate_exact_size(vec2(w, w * 9.0 / 16.0), Sense::click());
                    register(ui, format!("result:{salt}:{id}:{}", ms.unwrap_or(0)), r);
                    let shown = x["frame"].as_str().is_some_and(|f| paint_picture(app, ui, r, f, None));
                    if !shown {
                        let pid = lightcraft_catalog::PhotoId(id);
                        super::grid::request_thumb(app, pid, 256, 8);
                        if let Some(tex) = app.renderer.thumb(pid) {
                            let [tw, th] = tex.size;
                            let k = (r.width() / tw.max(1) as f32).min(r.height() / th.max(1) as f32);
                            let inner = Rect::from_center_size(r.center(), vec2(tw as f32 * k, th as f32 * k));
                            ui.painter().image(tex.tex.id(), inner, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        } else {
                            ui.painter().rect_filled(r, 3.0, t.canvas);
                        }
                    }
                    if resp.hovered() {
                        ui.painter().rect_stroke(r, 3.0, Stroke::new(1.5, t.accent), StrokeKind::Outside);
                    }
                    if resp.clicked() {
                        open_moment(app, id, ms);
                    }
                    ui.horizontal(|ui| {
                        let kind = if ms.is_some() { Icon::Video } else { Icon::Photos };
                        let (ir, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        paint(ui.painter(), ir, kind, t.icon);
                        if let Some(ms) = ms {
                            ui.label(egui::RichText::new(timecode(ms)).font(t.semibold(11.5)).color(t.text));
                        }
                        if let Some(st) = x["status"].as_str() {
                            let (text, c) = if st == "confirmed" { (tr("Confirmed"), t.accent) } else { (tr("Proposed"), t.caution) };
                            ui.label(egui::RichText::new(text).font(t.font(10.5)).color(c));
                        }
                    });
                    let people: Vec<&str> = x["people"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                    if !people.is_empty() {
                        ui.label(egui::RichText::new(people.join(", ")).font(t.semibold(11.0)).color(t.text_label));
                    }
                    let what =
                        [x["action"].as_str(), x["scene"].as_str()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                    if !what.is_empty() {
                        let g = crate::widgets::one_line(ui.painter(), &what, t.font(11.0), t.text_dim, w);
                        let (lr, _) = ui.allocate_exact_size(vec2(w, 16.0), Sense::hover());
                        ui.painter().galley(lr.left_center() - vec2(0.0, g.size().y / 2.0), g, t.text_dim);
                    }
                });
            }
        });
        ui.add_space(12.0);
    }
}
