//! Depth and inpainting: the engine side of Lens Blur, the Depth Range mask and object removal.
//!
//! - **Depth** (`depth.estimate`): Depth Anything V2 Small (`lightcraft-depth`, cargo feature
//!   `depth`) makes a relative depth map of a photo as developed. It is kept in a file next to the
//!   library (`depth/<content hash>.lcd`), never in the catalog's journal, and attached to the
//!   settings of every render that needs it (Lens Blur, Depth Range masks): rendering never needs
//!   the model.
//! - **Inpainting** (`spot.inpaint`): MI-GAN (`lightcraft-inpaint`, feature `inpaint`) fills a
//!   selection. The fill is kept in a file (`inpaint/<id>.lcp`); the edit is a [`Spot`] in
//!   [`SpotMode::Inpaint`] that names it, so it is undoable and the settings stay small.
//!
//! **Neither model is part of LightCraft.** Each is downloaded only when the user asks, after
//! being shown its licence (`depth.model.download {"acknowledged": true}`,
//! `inpaint.model.download`), with the same verified downloader as SAM 3. Without a model every
//! other feature works, and photos that already have their depth map or fill render the same.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightcraft_catalog::Photo;
#[cfg(feature = "depth")]
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{Aux, DepthMap, DevelopSettings, InpaintPatch, MaskShape};
#[cfg(feature = "inpaint")]
use lightcraft_develop::{Spot, SpotMode};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use crate::cmd::{CommandSpec, always, bad, cmd, f64_or, has_active, point};
use crate::{EngineError, Result, Session};

/// Long edge of the developed photo the depth model is shown.
pub const DEPTH_INPUT_EDGE: usize = 1600;
/// Long edge of the working copy an inpainting runs on (a preview-level source).
pub const INPAINT_EDGE: usize = 2560;

#[cfg(feature = "depth")]
pub const DEPTH_BYTES: u64 = lightcraft_depth::fetch::WEIGHTS_SIZE;
#[cfg(not(feature = "depth"))]
pub const DEPTH_BYTES: u64 = 0;
#[cfg(feature = "depth")]
pub const DEPTH_LICENSE: (&str, &str) = (lightcraft_depth::fetch::LICENSE_NAME, lightcraft_depth::fetch::LICENSE_URL);
#[cfg(not(feature = "depth"))]
pub const DEPTH_LICENSE: (&str, &str) = ("", "");
#[cfg(feature = "inpaint")]
pub const INPAINT_BYTES: u64 = lightcraft_inpaint::fetch::MODEL_SIZE;
#[cfg(not(feature = "inpaint"))]
pub const INPAINT_BYTES: u64 = 0;
#[cfg(feature = "inpaint")]
pub const INPAINT_LICENSE: (&str, &str) = (lightcraft_inpaint::fetch::LICENSE_NAME, lightcraft_inpaint::fetch::LICENSE_URL);
#[cfg(not(feature = "inpaint"))]
pub const INPAINT_LICENSE: (&str, &str) = ("", "");

/// The depth maps and inpainted pixels in memory, the models, and their downloads.
#[derive(Default)]
pub struct Ai {
    /// Where the Depth Anything V2 Small checkpoint lives (`None`: the default, see [`Ai::depth_dir`]).
    pub depth_dir: Option<PathBuf>,
    /// Where `migan.onnx` lives (`None`: the default, see [`Ai::inpaint_dir`]).
    pub inpaint_dir: Option<PathBuf>,
    depth_maps: HashMap<String, Arc<DepthMap>>,
    patches: HashMap<u64, Arc<InpaintPatch>>,
    #[cfg(feature = "depth")]
    depth_model: Option<Arc<lightcraft_depth::DepthModel>>,
    #[cfg(feature = "inpaint")]
    inpaint_model: Option<Arc<lightcraft_inpaint::Inpainter>>,
    #[cfg(any(feature = "depth", feature = "inpaint"))]
    depth_dl: crate::segment::download::Downloader,
    #[cfg(any(feature = "depth", feature = "inpaint"))]
    inpaint_dl: crate::segment::download::Downloader,
}

fn model_dir(env: &str, sub: &str, set: &Option<PathBuf>) -> Option<PathBuf> {
    set.clone()
        .or_else(|| std::env::var_os(env).map(PathBuf::from))
        .or_else(|| crate::camera_profiles::config_dir().map(|d| d.join("models").join(sub)))
}

impl Ai {
    pub const DEPTH_AVAILABLE: bool = cfg!(feature = "depth");
    pub const INPAINT_AVAILABLE: bool = cfg!(feature = "inpaint");

    pub fn depth_dir(&self) -> Option<PathBuf> {
        model_dir("LIGHTCRAFT_DEPTH_DIR", "depth", &self.depth_dir)
    }

    pub fn inpaint_dir(&self) -> Option<PathBuf> {
        model_dir("LIGHTCRAFT_INPAINT_DIR", "inpaint", &self.inpaint_dir)
    }

    /// Whether the depth model is in memory.
    pub fn depth_loaded(&self) -> bool {
        #[cfg(feature = "depth")]
        return self.depth_model.is_some();
        #[cfg(not(feature = "depth"))]
        false
    }

    /// Whether the inpainting model is in memory.
    pub fn inpaint_loaded(&self) -> bool {
        #[cfg(feature = "inpaint")]
        return self.inpaint_model.is_some();
        #[cfg(not(feature = "inpaint"))]
        false
    }

    pub fn depth_installed(&self) -> bool {
        #[cfg(feature = "depth")]
        if let Some(d) = self.depth_dir() {
            return lightcraft_depth::is_model_dir(&d);
        }
        false
    }

    pub fn inpaint_installed(&self) -> bool {
        #[cfg(feature = "inpaint")]
        if let Some(d) = self.inpaint_dir() {
            return lightcraft_inpaint::is_model_dir(&d);
        }
        false
    }
}

/// `{running, done, total, file, error, finished}` of a download.
#[cfg(any(feature = "depth", feature = "inpaint"))]
fn dl_json(d: &crate::segment::download::Downloader) -> Value {
    serde_json::to_value(d.status()).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------------------------
// Files next to the library, and attaching them to the settings a render uses

fn file_hash(key: &str) -> String {
    format!("{:032x}", lightcraft_preview::Hasher128::new().str(key).finish().0)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

impl Session {
    /// `<library>/<sub>/<name>` when the library is a folder on disk.
    fn ai_file(&self, sub: &str, name: &str) -> Option<PathBuf> {
        self.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join(sub).join(name))
    }

    /// The depth map of photo `p` (in memory, else from its file), if it has one.
    pub fn depth_map(&mut self, p: &Photo) -> Option<Arc<DepthMap>> {
        let key = crate::media::content_key(p);
        if let Some(d) = self.ai.depth_maps.get(&key) {
            return Some(d.clone());
        }
        let file = self.ai_file("depth", &format!("{}.lcd", file_hash(&key)))?;
        let d = Arc::new(DepthMap::from_bytes(&std::fs::read(file).ok()?)?);
        self.ai.depth_maps.insert(key, d.clone());
        Some(d)
    }

    /// Keep `d` as photo `p`'s depth map (in memory, and in a file with a library).
    pub fn store_depth_map(&mut self, p: &Photo, d: DepthMap) -> Result<Arc<DepthMap>> {
        let key = crate::media::content_key(p);
        if let Some(file) = self.ai_file("depth", &format!("{}.lcd", file_hash(&key))) {
            write_atomic(&file, &d.to_bytes()).map_err(|e| EngineError::Other(format!("{}: {e}", file.display())))?;
        }
        let d = Arc::new(d);
        self.ai.depth_maps.insert(key, d.clone());
        Ok(d)
    }

    fn patch(&mut self, hex: &str) -> Option<Arc<InpaintPatch>> {
        let id = u64::from_str_radix(hex, 16).ok()?;
        if let Some(p) = self.ai.patches.get(&id) {
            return Some(p.clone());
        }
        let file = self.ai_file("inpaint", &format!("{hex}.lcp"))?;
        let p = InpaintPatch::from_bytes(&std::fs::read(file).ok()?)?;
        (p.id == id).then(|| {
            let p = Arc::new(p);
            self.ai.patches.insert(id, p.clone());
            p
        })
    }

    /// Keep `p` in memory only (a preview's patch: no file is written).
    pub(crate) fn cache_patch(&mut self, p: InpaintPatch) -> Arc<InpaintPatch> {
        let p = Arc::new(p);
        self.ai.patches.insert(p.id, p.clone());
        p
    }

    #[cfg(test)]
    pub(crate) fn store_patch_for_test(&mut self, p: InpaintPatch) -> Arc<InpaintPatch> {
        self.store_patch(p).unwrap()
    }

    #[cfg_attr(not(feature = "inpaint"), allow(dead_code))]
    pub(crate) fn store_patch(&mut self, p: InpaintPatch) -> Result<Arc<InpaintPatch>> {
        if let Some(file) = self.ai_file("inpaint", &format!("{}.lcp", p.hex_id())) {
            write_atomic(&file, &p.to_bytes()).map_err(|e| EngineError::Other(format!("{}: {e}", file.display())))?;
        }
        let p = Arc::new(p);
        self.ai.patches.insert(p.id, p.clone());
        Ok(p)
    }

    /// `s` with the render-time data its edits refer to attached (see [`Aux`]): the photo's depth
    /// map when Lens Blur or a Depth Range mask is used, and the pixels of its inpainted spots.
    /// Returns `s` itself when nothing is needed or nothing exists yet.
    pub(crate) fn attach_aux(&mut self, p: &Photo, s: Arc<DevelopSettings>) -> Arc<DevelopSettings> {
        let wants_depth = s.lens_blur.enabled || s.masks.iter().any(|m| m.components.iter().any(|c| matches!(c.shape, MaskShape::DepthRange { .. })));
        let wants_patches = s.spots.iter().any(|sp| sp.patch.is_some());
        if !wants_depth && !wants_patches {
            return s;
        }
        let depth = if wants_depth { self.depth_map(p) } else { None };
        let patches: Vec<Arc<InpaintPatch>> = s.spots.iter().filter_map(|sp| sp.patch.clone()).filter_map(|hex| self.patch(&hex)).collect();
        if depth.is_none() && patches.is_empty() {
            return s;
        }
        let mut n = (*s).clone();
        n.aux = Aux { depth, patches };
        Arc::new(n)
    }
}

// ---------------------------------------------------------------------------------------------
// Depth

#[cfg(feature = "depth")]
impl Session {
    fn depth_model(&mut self) -> Result<Arc<lightcraft_depth::DepthModel>> {
        if let Some(m) = &self.ai.depth_model {
            return Ok(m.clone());
        }
        let dir = self.ai.depth_dir().ok_or_else(|| EngineError::Other("no folder is set for the depth model".into()))?;
        if !lightcraft_depth::is_model_dir(&dir) {
            return Err(EngineError::Other(format!(
                "The depth model is not installed. Download it (about {:.0} MB, {}) with `depth.model.download {{\"acknowledged\": true}}`, or put model.safetensors from {} in {}.",
                DEPTH_BYTES as f64 / 1e6,
                DEPTH_LICENSE.0,
                DEPTH_LICENSE.1,
                dir.display()
            )));
        }
        let m = Arc::new(lightcraft_depth::DepthModel::load(&dir).map_err(|e| EngineError::Other(e.to_string()))?);
        self.ai.depth_model = Some(m.clone());
        Ok(m)
    }

    /// Estimate and keep the depth map of photo `id` as developed (uncropped, without masks and
    /// Lens Blur). Returns (map width, height, seconds, device).
    pub fn estimate_depth(&mut self, id: PhotoId) -> Result<(usize, usize, f64, String)> {
        let c = "depth.estimate";
        let p = self.catalog.photo(id).ok_or_else(|| bad(c, "no such photo"))?.clone();
        let model = self.depth_model()?;
        let mut d = (*p.develop).clone();
        d.masks.clear();
        d.lens_blur.enabled = false;
        let job = self.preview_job(id, DEPTH_INPUT_EDGE, DEPTH_INPUT_EDGE, false, &d).ok_or_else(|| bad(c, "no such photo"))?;
        let img = job.run().rendered.map_err(|e| bad(c, e))?.image;
        let rgb: Vec<u8> = img.data.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
        let t = web_time::Instant::now();
        let raw = model.estimate(&rgb, img.width, img.height, lightcraft_depth::DEFAULT_SIZE).map_err(|e| bad(c, e.to_string()))?;
        let secs = t.elapsed().as_secs_f64();
        let map = DepthMap::from_relative(raw.width, raw.height, &raw.data).ok_or_else(|| bad(c, "the model returned an unusable depth map"))?;
        let (w, h) = (map.width, map.height);
        self.store_depth_map(&p, map)?;
        let device = if model.device().is_cpu() { "cpu" } else { "gpu" };
        Ok((w, h, secs, device.to_string()))
    }
}

/// The depth map of the active photo as a 16-bit grayscale PNG (white = near).
#[cfg(feature = "depth")]
fn write_depth_png(d: &DepthMap, path: &str) -> std::result::Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), d.width as u32, d.height as u32);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Sixteen);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = d.data.iter().flat_map(|v| v.to_be_bytes()).collect();
    w.write_image_data(&bytes).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------
// Inpainting

/// The selection of a `spot.inpaint` call as a bitmap over a `w × h` working copy: `rect`
/// `[x0, y0, x1, y1]`, `ellipse` `{center, rx, ry}` (radii as fractions of the long edge), or
/// `points` (a brushed path, each point a disc of `size` × long edge), all normalized to the
/// photo. Returns the mask, the selection's pin (centre) and radius (fractions of the long edge).
#[cfg_attr(not(feature = "inpaint"), allow(dead_code))]
fn selection(p: &Value, w: usize, h: usize, c: &str) -> Result<(Vec<u8>, Point, f64)> {
    let long = w.max(h) as f64;
    let mut mask = vec![0u8; w * h];
    let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut grow = |x0: f64, y0: f64, x1: f64, y1: f64| bounds = (bounds.0.min(x0), bounds.1.min(y0), bounds.2.max(x1), bounds.3.max(y1));
    let mut any = false;
    if let Some(r) = p.get("rect").and_then(Value::as_array).filter(|r| r.len() == 4) {
        let v: Vec<f64> = r.iter().filter_map(Value::as_f64).collect();
        if v.len() == 4 && v[2] > v[0] && v[3] > v[1] {
            let (x0, y0, x1, y1) = (
                (v[0] * w as f64).floor().max(0.0) as usize,
                (v[1] * h as f64).floor().max(0.0) as usize,
                (v[2] * w as f64).ceil() as usize,
                (v[3] * h as f64).ceil() as usize,
            );
            for y in y0..y1.min(h) {
                mask[y * w + x0..y * w + x1.min(w)].fill(255);
            }
            grow(v[0], v[1], v[2], v[3]);
            any = true;
        }
    }
    if let Some(e) = p.get("ellipse") {
        let (Some(ce), rx, ry) = (point(e, "center"), f64_or(e, "rx", 0.0), f64_or(e, "ry", 0.0)) else {
            return Err(bad(c, "`ellipse` needs center, rx, ry"));
        };
        if rx > 0.0 && ry > 0.0 {
            let (cx, cy, rxp, ryp) = (ce.x * w as f64, ce.y * h as f64, rx * long, ry * long);
            for y in ((cy - ryp).floor().max(0.0) as usize)..((cy + ryp).ceil() as usize).min(h) {
                for x in ((cx - rxp).floor().max(0.0) as usize)..((cx + rxp).ceil() as usize).min(w) {
                    if ((x as f64 + 0.5 - cx) / rxp).powi(2) + ((y as f64 + 0.5 - cy) / ryp).powi(2) <= 1.0 {
                        mask[y * w + x] = 255;
                    }
                }
            }
            grow(ce.x - rx * long / w as f64, ce.y - ry * long / h as f64, ce.x + rx * long / w as f64, ce.y + ry * long / h as f64);
            any = true;
        }
    }
    if let Some(pts) = p.get("points").and_then(Value::as_array) {
        let r = f64_or(p, "size", 0.02).clamp(0.001, 0.25) * long;
        let pts: Vec<(f64, f64)> = pts.iter().filter_map(|q| Some((q.get(0)?.as_f64()? * w as f64, q.get(1)?.as_f64()? * h as f64))).collect();
        // discs along the path (spacing r/3)
        let mut centres: Vec<(f64, f64)> = Vec::new();
        for (i, a) in pts.iter().enumerate() {
            centres.push(*a);
            if let Some(b) = pts.get(i + 1) {
                let n = (((b.0 - a.0).hypot(b.1 - a.1)) / (r / 3.0)).ceil() as usize;
                centres.extend((1..n).map(|k| (a.0 + (b.0 - a.0) * k as f64 / n as f64, a.1 + (b.1 - a.1) * k as f64 / n as f64)));
            }
        }
        for (cx, cy) in &centres {
            for y in ((cy - r).floor().max(0.0) as usize)..((cy + r).ceil() as usize).min(h) {
                for x in ((cx - r).floor().max(0.0) as usize)..((cx + r).ceil() as usize).min(w) {
                    if (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy) <= r {
                        mask[y * w + x] = 255;
                    }
                }
            }
            grow((cx - r) / w as f64, (cy - r) / h as f64, (cx + r) / w as f64, (cy + r) / h as f64);
            any = true;
        }
    }
    if !any {
        return Err(bad(c, "nothing to remove: give `rect` [x0,y0,x1,y1], `ellipse` {center,rx,ry} or `points` [[x,y],…] (normalized, with `size`)"));
    }
    let centre = Point::new(((bounds.0 + bounds.2) / 2.0).clamp(0.0, 1.0), ((bounds.1 + bounds.3) / 2.0).clamp(0.0, 1.0));
    let radius = ((bounds.2 - bounds.0) * w as f64).hypot((bounds.3 - bounds.1) * h as f64) / 2.0 / long;
    Ok((mask, centre, radius))
}

#[cfg(feature = "inpaint")]
impl Session {
    fn inpaint_model(&mut self) -> Result<Arc<lightcraft_inpaint::Inpainter>> {
        if let Some(m) = &self.ai.inpaint_model {
            return Ok(m.clone());
        }
        let dir = self.ai.inpaint_dir().ok_or_else(|| EngineError::Other("no folder is set for the inpainting model".into()))?;
        if !lightcraft_inpaint::is_model_dir(&dir) {
            return Err(EngineError::Other(format!(
                "The inpainting model is not installed. Download it (about {:.0} MB, {}) with `inpaint.model.download {{\"acknowledged\": true}}`, or put migan.onnx from {} in {}.",
                INPAINT_BYTES as f64 / 1e6,
                INPAINT_LICENSE.0,
                INPAINT_LICENSE.1,
                dir.display()
            )));
        }
        let m = Arc::new(lightcraft_inpaint::Inpainter::load(&dir).map_err(|e| EngineError::Other(e.to_string()))?);
        self.ai.inpaint_model = Some(m.clone());
        Ok(m)
    }

    /// Fill the selection `p` of the active photo and add it as an inpaint spot (one undo step).
    fn inpaint_spot(&mut self, p: &Value) -> Result<Value> {
        let c = "spot.inpaint";
        let id = self.active().ok_or_else(|| bad(c, "no active photo"))?;
        let ph = self.catalog.photo(id).ok_or_else(|| bad(c, "no such photo"))?.clone();
        let model = self.inpaint_model()?;
        // the photo at the scene-linear stage, uncropped (what the fill will be composited into)
        let job = self.render_job(id, INPAINT_EDGE, INPAINT_EDGE, false, false).ok_or_else(|| bad(c, "no such photo"))?;
        let source = job.source.load_source().map_err(|e| bad(c, e))?;
        let info = source.info_or(job.info);
        let lin = lightcraft_pipeline::linear_stage(&source.image, &info, &ph.develop, INPAINT_EDGE);
        drop(source);
        let (w, h) = (lin.width, lin.height);
        let (mask, centre, radius) = selection(p, w, h, c)?;
        let rgb = lightcraft_pipeline::inpaint::to_srgb8(&lin);
        let t = web_time::Instant::now();
        let fill = model.fill(&rgb, w, h, &mask, &lightcraft_inpaint::Options::default()).map_err(|e| bad(c, e.to_string()))?;
        let secs = t.elapsed().as_secs_f64();
        let rect =
            [fill.x as f64 / w as f64, fill.y as f64 / h as f64, (fill.x + fill.width) as f64 / w as f64, (fill.y + fill.height) as f64 / h as f64];
        let patch =
            InpaintPatch::new(rect, fill.width, fill.height, fill.rgb, fill.alpha).ok_or_else(|| bad(c, "the model returned an unusable fill"))?;
        let patch = self.store_patch(patch)?;
        let mut d = (*ph.develop).clone();
        d.spots.push(Spot {
            mode: SpotMode::Inpaint,
            points: vec![centre],
            size: radius.clamp(0.002, 0.5),
            feather: 0.0,
            opacity: 100.0,
            source_offset: None,
            patch: Some(patch.hex_id()),
        });
        let n = d.spots.len();
        self.set_develop(id, d, "Remove (inpaint)")?;
        self.active_spot = Some(n - 1);
        Ok(json!({"index": n - 1, "patch": patch.hex_id(), "seconds": secs, "fill": {"width": fill.width, "height": fill.height}}))
    }
}

// ---------------------------------------------------------------------------------------------
// Commands

#[cfg_attr(all(feature = "depth", feature = "inpaint"), allow(dead_code))]
fn not_built(what: &str) -> EngineError {
    EngineError::Other(format!("{what} is not available in this build"))
}

#[cfg(any(feature = "depth", feature = "inpaint"))]
fn mirrors_for(env: &str, which: &str) -> Vec<String> {
    let env = std::env::var(env).ok();
    let file = crate::camera_profiles::config_dir().map(|d| d.join("models").join(format!("{which}-mirrors.txt")));
    match which {
        #[cfg(feature = "depth")]
        "depth" => lightcraft_depth::fetch::mirrors(env.as_deref(), file.as_deref()),
        #[cfg(feature = "inpaint")]
        "inpaint" => lightcraft_inpaint::fetch::mirrors(env.as_deref(), file.as_deref()),
        _ => Vec::new(),
    }
}

fn ack(p: &Value, c: &str, what: &str, bytes: u64, licence: (&str, &str)) -> Result<()> {
    if p.get("acknowledged").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    Err(bad(
        c,
        format!(
            "the {what} model is a {:.0} MB download under the {} ({}): pass `acknowledged: true` once the user has agreed to download it",
            bytes as f64 / 1e6,
            licence.0,
            licence.1
        ),
    ))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "depth.model.status",
            "Depth Model Status",
            [],
            None,
            "{} — whether this build can estimate depth, whether the Depth Anything V2 Small model is installed (and where), and the download's progress → {available, installed, dir, loaded, sizeBytes, license, licenseUrl, mirrors, download}",
            always,
            |s, _| {
                Ok(json!({
                    "available": Ai::DEPTH_AVAILABLE,
                    "installed": s.ai.depth_installed(),
                    "dir": s.ai.depth_dir().map(|d| d.display().to_string()),
                    "loaded": s.ai.depth_loaded(),
                    "sizeBytes": DEPTH_BYTES,
                    "license": DEPTH_LICENSE.0,
                    "licenseUrl": DEPTH_LICENSE.1,
                    "mirrors": depth_mirrors(),
                    "download": depth_download(s),
                }))
            }
        ),
        cmd!(
            query "depth.model.download",
            "Download Depth Model",
            [],
            None,
            "{acknowledged: true} — download Depth Anything V2 Small (about 99 MB, Apache-2.0; the larger V2 models are non-commercial and are not used) in the background, verified by SHA-256; only after the user agreed. Watch depth.model.status; depth.model.cancel stops it → {started, installed, downloading}",
            always,
            |s, p| {
                let c = "depth.model.download";
                ack(p, c, "depth", DEPTH_BYTES, DEPTH_LICENSE)?;
                start_depth_download(s)
            }
        ),
        cmd!(
            query "depth.model.cancel",
            "Cancel Depth Model Download",
            [],
            None,
            "{} — stop the depth model download (what has arrived is kept) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": cancel_depth_download(s)}))
        ),
        cmd!(
            "depth.estimate",
            "Estimate Depth",
            [],
            None,
            "{ids?} — estimate the depth of the photos (the selection by default) with Depth Anything V2 Small, as developed; kept next to the library, used by Lens Blur and the Depth Range mask; needs the model (depth.model.download) → {photos: [{id, width, height, seconds, device}]}",
            crate::cmd::has_selection,
            |s, p| depth_estimate(s, p)
        ),
        cmd!(
            query "depth.export",
            "Export Depth Map",
            [],
            None,
            "{path} — write the active photo's depth map as a 16-bit grayscale PNG (white = near) → {width, height}",
            has_active,
            |s, p| depth_export(s, p)
        ),
        cmd!(
            "lensblur.set",
            "Lens Blur",
            [],
            None,
            "{enabled?, amount?: 0..100 (blur radius: 2 % of the long edge at 100), focus?: 0..100 (depth kept sharp, 0 far … 100 near), point?: [x,y] (normalized: focus on what is there), range?: 0..100 (depth band kept sharp)} — Lens Blur on the active photo (disc-shaped aperture blur by depth, one undo step); needs the photo's depth map (depth.estimate), and is on by default when amount > 0 → {lensBlur}",
            has_active,
            |s, p| lensblur_set(s, p)
        ),
        cmd!(
            "spot.inpaint",
            "Remove by Inpainting",
            [],
            None,
            "{rect?: [x0,y0,x1,y1], ellipse?: {center:[x,y], rx, ry}, points?: [[x,y],…], size?: fraction of long edge for points (0.02)} — fill the selection (normalized photo coordinates) with MI-GAN, a spot in inpaint mode (one undo step; the fill is kept next to the library); needs the model (inpaint.model.download) → {index, patch, seconds}",
            has_active,
            |s, p| spot_inpaint(s, p)
        ),
        cmd!(
            query "inpaint.model.status",
            "Inpainting Model Status",
            [],
            None,
            "{} — whether this build can inpaint, whether the MI-GAN model is installed (and where), and the download's progress → {available, installed, dir, loaded, sizeBytes, license, licenseUrl, mirrors, download}",
            always,
            |s, _| {
                Ok(json!({
                    "available": Ai::INPAINT_AVAILABLE,
                    "installed": s.ai.inpaint_installed(),
                    "dir": s.ai.inpaint_dir().map(|d| d.display().to_string()),
                    "loaded": s.ai.inpaint_loaded(),
                    "sizeBytes": INPAINT_BYTES,
                    "license": INPAINT_LICENSE.0,
                    "licenseUrl": INPAINT_LICENSE.1,
                    "mirrors": inpaint_mirrors(),
                    "download": inpaint_download(s),
                }))
            }
        ),
        cmd!(
            query "inpaint.model.download",
            "Download Inpainting Model",
            [],
            None,
            "{acknowledged: true} — download MI-GAN (about 30 MB, MIT licence; trained on Places2 and FFHQ) in the background, verified by SHA-256; only after the user agreed. Watch inpaint.model.status; inpaint.model.cancel stops it → {started, installed, downloading}",
            always,
            |s, p| {
                let c = "inpaint.model.download";
                ack(p, c, "inpainting", INPAINT_BYTES, INPAINT_LICENSE)?;
                start_inpaint_download(s)
            }
        ),
        cmd!(
            query "inpaint.model.cancel",
            "Cancel Inpainting Model Download",
            [],
            None,
            "{} — stop the inpainting model download (what has arrived is kept) → {cancelled}",
            always,
            |s, _| Ok(json!({"cancelled": cancel_inpaint_download(s)}))
        ),
    ]
}

// ----- depth

fn depth_mirrors() -> usize {
    #[cfg(feature = "depth")]
    return mirrors_for("LIGHTCRAFT_DEPTH_MIRRORS", "depth").len();
    #[cfg(not(feature = "depth"))]
    0
}

fn inpaint_mirrors() -> usize {
    #[cfg(feature = "inpaint")]
    return mirrors_for("LIGHTCRAFT_INPAINT_MIRRORS", "inpaint").len();
    #[cfg(not(feature = "inpaint"))]
    0
}

fn depth_download(s: &Session) -> Value {
    #[cfg(feature = "depth")]
    return dl_json(&s.ai.depth_dl);
    #[cfg(not(feature = "depth"))]
    {
        let _ = s;
        Value::Null
    }
}

fn inpaint_download(s: &Session) -> Value {
    #[cfg(feature = "inpaint")]
    return dl_json(&s.ai.inpaint_dl);
    #[cfg(not(feature = "inpaint"))]
    {
        let _ = s;
        Value::Null
    }
}

fn start_depth_download(s: &mut Session) -> Result<Value> {
    #[cfg(feature = "depth")]
    {
        let dir = s.ai.depth_dir().ok_or_else(|| EngineError::Other("no folder is set for the depth model".into()))?;
        if s.ai.depth_installed() {
            return Ok(json!({"started": false, "installed": true, "downloading": false}));
        }
        let m = mirrors_for("LIGHTCRAFT_DEPTH_MIRRORS", "depth");
        let started =
            s.ai.depth_dl.start(lightcraft_depth::fetch::FILES, m, dir, lightcraft_fetch::Options::default()).map_err(EngineError::Other)?;
        Ok(json!({"started": started, "installed": false, "downloading": s.ai.depth_dl.running()}))
    }
    #[cfg(not(feature = "depth"))]
    {
        let _ = s;
        Err(not_built("Depth estimation"))
    }
}

fn start_inpaint_download(s: &mut Session) -> Result<Value> {
    #[cfg(feature = "inpaint")]
    {
        let dir = s.ai.inpaint_dir().ok_or_else(|| EngineError::Other("no folder is set for the inpainting model".into()))?;
        if s.ai.inpaint_installed() {
            return Ok(json!({"started": false, "installed": true, "downloading": false}));
        }
        let m = mirrors_for("LIGHTCRAFT_INPAINT_MIRRORS", "inpaint");
        let started =
            s.ai.inpaint_dl.start(lightcraft_inpaint::fetch::FILES, m, dir, lightcraft_fetch::Options::default()).map_err(EngineError::Other)?;
        Ok(json!({"started": started, "installed": false, "downloading": s.ai.inpaint_dl.running()}))
    }
    #[cfg(not(feature = "inpaint"))]
    {
        let _ = s;
        Err(not_built("Inpainting"))
    }
}

fn cancel_depth_download(s: &Session) -> bool {
    #[cfg(feature = "depth")]
    return s.ai.depth_dl.cancel();
    #[cfg(not(feature = "depth"))]
    {
        let _ = s;
        false
    }
}

fn cancel_inpaint_download(s: &Session) -> bool {
    #[cfg(feature = "inpaint")]
    return s.ai.inpaint_dl.cancel();
    #[cfg(not(feature = "inpaint"))]
    {
        let _ = s;
        false
    }
}

fn depth_estimate(s: &mut Session, p: &Value) -> Result<Value> {
    #[cfg(feature = "depth")]
    {
        let mut out = Vec::new();
        for id in s.targets(p) {
            let (w, h, secs, device) = s.estimate_depth(id)?;
            out.push(json!({"id": id.0, "width": w, "height": h, "seconds": secs, "device": device}));
        }
        Ok(json!({"photos": out}))
    }
    #[cfg(not(feature = "depth"))]
    {
        let _ = (s, p);
        Err(not_built("Depth estimation"))
    }
}

fn depth_export(s: &mut Session, p: &Value) -> Result<Value> {
    #[cfg(feature = "depth")]
    {
        let c = "depth.export";
        let path = p.get("path").and_then(Value::as_str).ok_or_else(|| bad(c, "missing `path`"))?;
        let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
        let ph = s.catalog.photo(id).ok_or_else(|| bad(c, "no such photo"))?.clone();
        let d = s.depth_map(&ph).ok_or_else(|| bad(c, "this photo has no depth map yet (depth.estimate)"))?;
        write_depth_png(&d, path).map_err(|e| bad(c, e))?;
        Ok(json!({"width": d.width, "height": d.height}))
    }
    #[cfg(not(feature = "depth"))]
    {
        let _ = (s, p);
        Err(not_built("Depth estimation"))
    }
}

fn lensblur_set(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "lensblur.set";
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let ph = s.catalog.photo(id).ok_or_else(|| bad(c, "no such photo"))?.clone();
    let mut d = (*ph.develop).clone();
    let lb = &mut d.lens_blur;
    if let Some(a) = p.get("amount").and_then(Value::as_f64) {
        lb.amount = a.clamp(0.0, 100.0);
        lb.enabled = lb.amount > 0.0;
    }
    if let Some(e) = p.get("enabled").and_then(Value::as_bool) {
        lb.enabled = e;
    }
    if let Some(f) = p.get("focus").and_then(Value::as_f64) {
        lb.focal_distance = f.clamp(0.0, 100.0);
    }
    if let Some(r) = p.get("range").and_then(Value::as_f64) {
        lb.focal_range = r.clamp(0.0, 100.0);
    }
    if let Some(pt) = point(p, "point") {
        let map = s.depth_map(&ph).ok_or_else(|| bad(c, "this photo has no depth map yet (depth.estimate)"))?;
        d.lens_blur.focal_distance = f64::from(map.at(pt.x.clamp(0.0, 1.0), pt.y.clamp(0.0, 1.0))) * 100.0;
    }
    let out = json!({"lensBlur": d.lens_blur, "hasDepth": s.depth_map(&ph).is_some()});
    s.set_develop(id, d, "Lens Blur")?;
    Ok(out)
}

fn spot_inpaint(s: &mut Session, p: &Value) -> Result<Value> {
    #[cfg(feature = "inpaint")]
    return s.inpaint_spot(p);
    #[cfg(not(feature = "inpaint"))]
    {
        let _ = (s, p);
        Err(not_built("Inpainting"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_become_masks_and_pins() {
        let (m, c, r) = selection(&json!({"rect": [0.25, 0.25, 0.75, 0.5]}), 100, 80, "t").unwrap();
        assert_eq!(m.iter().filter(|v| **v == 255).count(), 50 * 20);
        assert!((c.x - 0.5).abs() < 1e-9 && (c.y - 0.375).abs() < 1e-9 && r > 0.0);
        let (m, _, _) = selection(&json!({"ellipse": {"center": [0.5, 0.5], "rx": 0.1, "ry": 0.05}}), 100, 100, "t").unwrap();
        let area = m.iter().filter(|v| **v == 255).count() as f64;
        assert!((area - std::f64::consts::PI * 10.0 * 5.0).abs() < 12.0, "{area}");
        let (m, c, _) = selection(&json!({"points": [[0.2, 0.5], [0.8, 0.5]], "size": 0.02}), 100, 100, "t").unwrap();
        assert!(m[50 * 100 + 50] == 255 && m[10 * 100 + 10] == 0 && (c.x - 0.5).abs() < 0.02);
        assert!(selection(&json!({}), 10, 10, "t").is_err());
    }
}
