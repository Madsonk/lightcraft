//! Automatic masks from two optional networks (`lightcraft-autoseg`): SegFormer for Sky / People /
//! Landscape classes and an ISNet matting network for the Subject. The result is stored with the
//! mask as a [`SegMask`] (like SAM 3's), so rendering never needs a model.
//!
//! **The models are optional and not part of LightCraft**: they are downloaded only when the user
//! asks (`autoseg.model.download`, after naming the licence), verified by size and SHA-256. Without
//! the build feature `autoseg` the commands report that this build has no automatic masks.

use std::path::PathBuf;

/// Whether this build can run the networks at all.
pub const AVAILABLE: bool = cfg!(feature = "autoseg");

static OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Use `dir` as the models folder (tests, front ends with their own settings); `None` restores the default.
pub fn set_models_dir(dir: Option<PathBuf>) {
    *OVERRIDE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = dir;
}

/// The folder holding the model folders: `LIGHTCRAFT_AUTOSEG_DIR`, else `models/autoseg` in the
/// settings folder.
pub fn models_dir() -> Option<PathBuf> {
    if let Some(d) = OVERRIDE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() {
        return Some(d);
    }
    std::env::var_os("LIGHTCRAFT_AUTOSEG_DIR")
        .map(PathBuf::from)
        .or_else(|| crate::camera_profiles::config_dir().map(|d| d.join("models").join("autoseg")))
}

/// Whether at least one automatic-mask model is installed (the Masking panel says so).
pub fn models_installed() -> bool {
    #[cfg(feature = "autoseg")]
    return imp::Model::ALL.into_iter().any(imp::installed);
    #[cfg(not(feature = "autoseg"))]
    false
}

#[cfg(feature = "autoseg")]
pub use imp::{Kind, Model, cancel_download, installed, models, segment, start_download, status};

#[cfg(feature = "autoseg")]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use lightcraft_autoseg::files::{self, ALL};
    use lightcraft_autoseg::{isnet, segformer};
    use lightcraft_develop::SegMask;
    use lightcraft_raster::Rgba8;
    use serde_json::{Value, json};

    use super::models_dir;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Model {
        Segformer,
        Subject,
    }

    impl Model {
        pub const ALL: [Model; 2] = [Model::Segformer, Model::Subject];
        pub fn id(self) -> &'static str {
            self.spec().id
        }
        pub fn parse(s: &str) -> Option<Model> {
            Self::ALL.into_iter().find(|m| m.id() == s)
        }
        pub fn spec(self) -> &'static files::Model {
            ALL[self as usize]
        }
        pub fn dir(self) -> Option<std::path::PathBuf> {
            models_dir().map(|d| d.join(self.spec().dir))
        }
    }

    /// What to select.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Kind {
        /// ADE20K classes by group name (`sky`, `people`, `vegetation`, `water`, `mountain`, `buildings`, `ground`).
        Class(String),
        Subject,
    }

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn installed(m: Model) -> bool {
        m.dir().is_some_and(|d| m.spec().files.iter().filter(|f| f.sha256.is_some()).all(|f| d.join(f.name).is_file()))
    }

    static SEGFORMER: Mutex<Option<Arc<segformer::SegFormer>>> = Mutex::new(None);
    static ISNET: Mutex<Option<Arc<isnet::IsNet>>> = Mutex::new(None);

    fn missing(m: Model) -> String {
        let spec = m.spec();
        let dir = m.dir().map_or_else(|| "the models folder".to_string(), |d| d.display().to_string());
        let size: u64 = spec.files.iter().filter_map(|f| f.size).sum();
        format!(
            "The {} model is not installed. It is not part of LightCraft: download it ({:.0} MB; {}) with \
             `autoseg.model.download {{\"model\": \"{}\", \"acknowledged\": true}}` after agreeing to its licence ({}), or put its files in {dir}.",
            spec.title,
            size as f64 / 1e6,
            spec.licence,
            m.id(),
            spec.licence_url
        )
    }

    fn segformer_model() -> Result<Arc<segformer::SegFormer>, String> {
        let mut g = lock(&SEGFORMER);
        if let Some(m) = &*g {
            return Ok(m.clone());
        }
        if !installed(Model::Segformer) {
            return Err(missing(Model::Segformer));
        }
        let dir = Model::Segformer.dir().ok_or("no folder is set for the models")?;
        let m = Arc::new(segformer::SegFormer::load(&dir).map_err(|e| format!("SegFormer: {e}"))?);
        *g = Some(m.clone());
        Ok(m)
    }

    fn isnet_model() -> Result<Arc<isnet::IsNet>, String> {
        let mut g = lock(&ISNET);
        if let Some(m) = &*g {
            return Ok(m.clone());
        }
        if !installed(Model::Subject) {
            return Err(missing(Model::Subject));
        }
        let dir = Model::Subject.dir().ok_or("no folder is set for the models")?;
        let m = Arc::new(isnet::IsNet::load(&dir).map_err(|e| format!("subject model: {e}"))?);
        *g = Some(m.clone());
        Ok(m)
    }

    /// Side of the stored segmentation grid for classes (the network's own grid is coarser).
    const CLASS_SIDE: usize = 512;

    /// `img` stretched to `w x h` as RGB: an area average (what the source cells cover, with
    /// fractional edges), which does not alias when the photo is much larger than the network's input.
    fn stretch(img: &Rgba8, w: usize, h: usize) -> Vec<u8> {
        let (sw, sh) = (img.width, img.height);
        let axis = |n_src: usize, n_dst: usize| -> Vec<Vec<(usize, f32)>> {
            let k = n_src as f32 / n_dst as f32;
            (0..n_dst)
                .map(|d| {
                    let (a, b) = (d as f32 * k, (d as f32 + 1.0) * k);
                    // upscaling: the one or two source cells around the centre
                    let (a, b) = if k < 1.0 {
                        let c = (a + b) / 2.0;
                        (c - 0.5, c + 0.5)
                    } else {
                        (a, b)
                    };
                    let (i0, i1) = ((a.floor().max(0.0)) as usize, ((b.ceil()) as usize).min(n_src));
                    let mut v: Vec<(usize, f32)> =
                        (i0..i1.max(i0 + 1)).map(|i| (i.min(n_src - 1), ((i as f32 + 1.0).min(b) - (i as f32).max(a)).max(0.0))).collect();
                    let t: f32 = v.iter().map(|x| x.1).sum();
                    if t <= 0.0 {
                        v = vec![(i0.min(n_src - 1), 1.0)];
                    } else {
                        v.iter_mut().for_each(|x| x.1 /= t);
                    }
                    v
                })
                .collect()
        };
        let (xs, ys) = (axis(sw, w), axis(sh, h));
        let mut out = vec![0u8; w * h * 3];
        for (y, wy) in ys.iter().enumerate() {
            for (x, wx) in xs.iter().enumerate() {
                let mut acc = [0f32; 3];
                for (j, fy) in wy {
                    for (i, fx) in wx {
                        let p = img.data[j * sw + i];
                        for c in 0..3 {
                            acc[c] += f32::from(p[c]) * fx * fy;
                        }
                    }
                }
                for c in 0..3 {
                    out[(y * w + x) * 3 + c] = (acc[c] + 0.5).clamp(0.0, 255.0) as u8;
                }
            }
        }
        out
    }

    /// Bilinear resampling of a `gw x gh` plane to `side x side` (cell centres).
    fn to_square(v: &[f32], gw: usize, gh: usize, side: usize) -> Vec<f32> {
        let mut out = vec![0f32; side * side];
        for y in 0..side {
            let fy = ((y as f32 + 0.5) * gh as f32 / side as f32 - 0.5).clamp(0.0, gh as f32 - 1.0);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(gh - 1);
            for x in 0..side {
                let fx = ((x as f32 + 0.5) * gw as f32 / side as f32 - 0.5).clamp(0.0, gw as f32 - 1.0);
                let (x0, tx) = (fx.floor() as usize, fx.fract());
                let x1 = (x0 + 1).min(gw - 1);
                let a = v[y0 * gw + x0] * (1.0 - tx) + v[y0 * gw + x1] * tx;
                let b = v[y1 * gw + x0] * (1.0 - tx) + v[y1 * gw + x1] * tx;
                out[y * side + x] = a * (1.0 - ty) + b * ty;
            }
        }
        out
    }

    /// Segment `img` (the developed photo, uncropped, display-encoded) for `kind`. Returns the
    /// stored segmentation and the seconds the network took.
    pub fn segment(img: &Rgba8, kind: &Kind) -> Result<(SegMask, f64), String> {
        let t = web_time::Instant::now();
        let mask = match kind {
            Kind::Class(name) => {
                let classes = segformer::landscape_classes(name)
                    .ok_or_else(|| format!("unknown class group `{name}` (sky, people, vegetation, water, mountain, buildings, ground)"))?;
                let m = segformer_model()?;
                let s = 512.0 / img.width.min(img.height).max(1) as f32;
                let r32 = |v: usize| (((v as f32 * s) / 32.0).round() as usize).max(1) * 32;
                let (w, h) = (r32(img.width), r32(img.height));
                let rgb = stretch(img, w, h);
                let (logits, gw, gh) = m.forward(&rgb, w, h).map_err(|e| format!("SegFormer: {e}"))?;
                let l = segformer::class_logit(&logits, gw * gh, classes);
                SegMask::from_logits(CLASS_SIDE, &to_square(&l, gw, gh, CLASS_SIDE))
            }
            Kind::Subject => {
                let m = isnet_model()?;
                let rgb = stretch(img, isnet::SIDE, isnet::SIDE);
                let p = m.matte(&rgb).map_err(|e| format!("subject model: {e}"))?;
                let l: Vec<f32> = p.into_iter().map(lightcraft_autoseg::logit).collect();
                SegMask::from_logits(isnet::SIDE, &l)
            }
        };
        Ok((mask, t.elapsed().as_secs_f64()))
    }

    // ---- downloads (background threads; never block a session) ----

    struct Download {
        running: AtomicBool,
        cancel: AtomicBool,
        finished: AtomicBool,
        done: AtomicU64,
        total: AtomicU64,
        error: Mutex<Option<String>>,
    }

    impl Download {
        const fn new() -> Self {
            Self {
                running: AtomicBool::new(false),
                cancel: AtomicBool::new(false),
                finished: AtomicBool::new(false),
                done: AtomicU64::new(0),
                total: AtomicU64::new(0),
                error: Mutex::new(None),
            }
        }
    }

    static DOWNLOADS: [Download; 2] = [Download::new(), Download::new()];

    pub const MIRRORS_ENV: &str = "LIGHTCRAFT_AUTOSEG_MIRRORS";

    /// Start downloading `m` in the background. `Ok(false)` when it is installed or downloading.
    pub fn start_download(m: Model) -> Result<bool, String> {
        let dir = m.dir().ok_or("no folder is set for the AI models")?;
        if installed(m) {
            return Ok(false);
        }
        let d = &DOWNLOADS[m as usize];
        if d.running.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        d.cancel.store(false, Ordering::SeqCst);
        d.finished.store(false, Ordering::SeqCst);
        d.done.store(0, Ordering::SeqCst);
        let spec = m.spec();
        d.total.store(spec.files.iter().filter_map(|f| f.size).sum(), Ordering::SeqCst);
        *lock(&d.error) = None;
        let mirrors = lightcraft_fetch::mirrors(std::env::var(MIRRORS_ENV).ok().as_deref(), None, &[spec.mirror]);
        let spawned = std::thread::Builder::new().name(format!("autoseg-download-{}", m.id())).spawn(move || {
            let d = &DOWNLOADS[m as usize];
            let mut on_progress = |p: &lightcraft_fetch::Progress| {
                d.done.store(p.done, Ordering::SeqCst);
                d.total.store(p.total, Ordering::SeqCst);
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                lightcraft_fetch::download(spec.files, &mirrors, &dir, &lightcraft_fetch::Options::default(), &d.cancel, &mut on_progress)
            }));
            let error = match r {
                Ok(Ok(())) => {
                    d.finished.store(true, Ordering::SeqCst);
                    None
                }
                Ok(Err(e)) => Some(e.to_string()),
                Err(_) => Some("the download failed unexpectedly".to_string()),
            };
            *lock(&d.error) = error;
            d.running.store(false, Ordering::SeqCst);
        });
        if let Err(e) = spawned {
            d.running.store(false, Ordering::SeqCst);
            return Err(format!("could not start the download: {e}"));
        }
        Ok(true)
    }

    pub fn cancel_download(m: Model) -> bool {
        let d = &DOWNLOADS[m as usize];
        let running = d.running.load(Ordering::SeqCst);
        if running {
            d.cancel.store(true, Ordering::SeqCst);
        }
        running
    }

    pub fn status(m: Model) -> Value {
        let (spec, d) = (m.spec(), &DOWNLOADS[m as usize]);
        json!({
            "id": m.id(),
            "title": spec.title,
            "files": spec.files.iter().map(|f| f.name).collect::<Vec<_>>(),
            "sizeBytes": spec.files.iter().filter_map(|f| f.size).sum::<u64>(),
            "installed": installed(m),
            "licence": spec.licence,
            "licenceUrl": spec.licence_url,
            "sourceUrl": spec.source_url,
            "download": {
                "running": d.running.load(Ordering::SeqCst),
                "done": d.done.load(Ordering::SeqCst),
                "total": d.total.load(Ordering::SeqCst),
                "finished": d.finished.load(Ordering::SeqCst),
                "error": lock(&d.error).clone(),
            },
        })
    }

    pub fn models() -> Vec<Value> {
        Model::ALL.into_iter().map(status).collect()
    }
}
