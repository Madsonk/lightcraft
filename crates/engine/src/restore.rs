//! AI denoise (SCUNet) and 2x super-resolution (SPAN) of a finished render, applied by exports
//! and the CLI's `render` (see `docs/ai-restore.md`).
//!
//! **The models are optional.** They are not part of LightCraft: they are downloaded only when
//! the user asks (`restore.model.download`, after naming the licence), from the places their
//! authors publish them, and verified by size and SHA-256. Without them nothing else changes;
//! an export that asks for AI denoise or super-resolution says so and stops, it never silently
//! skips the step. The networks run on the GPU (Metal) where there is one, else the CPU; they
//! take minutes on a full-size photo, which is why only exports run them, never the preview.
//!
//! The build feature `restore` gates it all; without it the commands report that this build has
//! no AI restore.

use std::path::PathBuf;

use lightcraft_pipeline::Rendered;

/// Whether this build can run the networks at all.
pub const AVAILABLE: bool = cfg!(feature = "restore");

/// What to apply to a render: AI denoise at `denoise` percent (0 = off) and 2x super-resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Request {
    pub denoise: f64,
    pub upscale: bool,
}

impl Request {
    pub fn from_settings(s: &lightcraft_develop::DevelopSettings) -> Self {
        Self { denoise: s.enhance.restore_denoise.clamp(0.0, 100.0), upscale: s.enhance.super_resolution }
    }

    pub fn is_noop(&self) -> bool {
        self.denoise <= 0.0 && !self.upscale
    }
}

/// The folder holding the model files: `LIGHTCRAFT_RESTORE_DIR`, else `models/restore` in the
/// settings folder.
pub fn models_dir() -> Option<PathBuf> {
    std::env::var_os("LIGHTCRAFT_RESTORE_DIR")
        .map(PathBuf::from)
        .or_else(|| crate::camera_profiles::config_dir().map(|d| d.join("models").join("restore")))
}

/// Whether the AI denoise model is in place (false in a build without AI restore).
pub fn denoise_installed() -> bool {
    #[cfg(feature = "restore")]
    return imp::installed(imp::Model::Denoise);
    #[cfg(not(feature = "restore"))]
    false
}

/// Apply `req` to `r` (the render of an export): AI denoise first (at the render's size, where the
/// noise is), then 2x super-resolution. Errors say what is missing and how to get it.
pub fn apply(r: Rendered, req: Request) -> Result<Rendered, String> {
    if req.is_noop() {
        return Ok(r);
    }
    imp::apply(r, req)
}

#[cfg(not(feature = "restore"))]
mod imp {
    use super::{Rendered, Request};

    pub fn apply(_: Rendered, _: Request) -> Result<Rendered, String> {
        Err("AI denoise and super-resolution are not available in this build".into())
    }
}

#[cfg(feature = "restore")]
pub use imp::{Model, cancel_download, denoise_backend, models, start_download, status};

#[cfg(feature = "restore")]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use lightcraft_pipeline::{DeepImage, DeepSamples, OutputTrc};
    use lightcraft_raster::{Histogram, Rgba8};
    use lightcraft_restore::{Denoiser, Image, Upscaler, files};
    use serde_json::{Value, json};

    #[allow(clippy::wildcard_imports)]
    use super::*;

    /// The two models.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Model {
        Denoise,
        Upscale,
    }

    pub const ALL: [Model; 2] = [Model::Denoise, Model::Upscale];

    impl Model {
        pub fn id(self) -> &'static str {
            match self {
                Model::Denoise => "denoise",
                Model::Upscale => "upscale",
            }
        }
        pub fn parse(s: &str) -> Option<Model> {
            ALL.into_iter().find(|m| m.id() == s)
        }
        pub fn spec(self) -> &'static files::Model {
            match self {
                Model::Denoise => &files::SCUNET,
                Model::Upscale => &files::SPAN2X,
            }
        }
        fn slot(self) -> usize {
            self as usize
        }
    }

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---- the loaded networks (kept for the next photo of a batch) ----

    static DENOISER: Mutex<Option<Arc<Denoiser>>> = Mutex::new(None);
    static UPSCALER: Mutex<Option<Arc<Upscaler>>> = Mutex::new(None);

    pub fn installed(m: Model) -> bool {
        super::models_dir().is_some_and(|d| d.join(m.spec().file.name).is_file())
    }

    fn missing(m: Model) -> String {
        let spec = m.spec();
        let what = match m {
            Model::Denoise => "AI denoise",
            Model::Upscale => "2x super-resolution",
        };
        let dir = super::models_dir().map_or_else(|| "the models folder".to_string(), |d| d.display().to_string());
        let size = spec.file.size.unwrap_or(0) as f64 / 1e6;
        let d = &DOWNLOADS[m.slot()];
        if d.running.load(Ordering::SeqCst) {
            let pct = d.done.load(Ordering::SeqCst).saturating_mul(100).checked_div(d.total.load(Ordering::SeqCst)).unwrap_or(0);
            return format!("The {what} model ({}) is not installed yet: it is downloading ({pct} %).", spec.title);
        }
        format!(
            "The {what} model ({}) is not installed. It is not part of LightCraft: download it ({size:.0} MB; {}) with \
             `restore.model.download {{\"model\": \"{}\", \"acknowledged\": true}}` after agreeing to its licence ({}), \
             or put {} in {dir}.",
            spec.title,
            spec.licence,
            m.id(),
            spec.licence_url,
            spec.file.name
        )
    }

    fn load_denoiser() -> Result<Arc<Denoiser>, String> {
        let mut g = lock(&DENOISER);
        if let Some(d) = &*g {
            return Ok(d.clone());
        }
        if !installed(Model::Denoise) {
            return Err(missing(Model::Denoise));
        }
        let path = super::models_dir().ok_or("no folder is set for the AI models")?.join(files::SCUNET.file.name);
        let d = Arc::new(Denoiser::load(&path).map_err(|e| format!("AI denoise model: {e}"))?);
        *g = Some(d.clone());
        Ok(d)
    }

    fn load_upscaler() -> Result<Arc<Upscaler>, String> {
        let mut g = lock(&UPSCALER);
        if let Some(u) = &*g {
            return Ok(u.clone());
        }
        if !installed(Model::Upscale) {
            return Err(missing(Model::Upscale));
        }
        let path = super::models_dir().ok_or("no folder is set for the AI models")?.join(files::SPAN2X.file.name);
        let u = Arc::new(Upscaler::load(&path).map_err(|e| format!("super-resolution model: {e}"))?);
        *g = Some(u.clone());
        Ok(u)
    }

    // ---- converting a render to and from the networks' domain (gamma-encoded 0..1) ----

    /// The render as the networks see it, and how to put a result back.
    pub(super) enum Pixels {
        Eight(Rgba8),
        Deep(DeepImage),
    }

    pub(super) fn to_image(p: &Pixels) -> Image {
        match p {
            Pixels::Eight(img) => Image {
                width: img.width,
                height: img.height,
                data: img.data.iter().flat_map(|p| [p[0], p[1], p[2]]).map(|v| v as f32 / 255.0).collect(),
            },
            Pixels::Deep(d) => {
                let data = match &d.samples {
                    DeepSamples::U16(v) => v.iter().map(|&x| x as f32 / 65535.0).collect(),
                    DeepSamples::F32(v) => {
                        let trc: OutputTrc = d.space.trc();
                        v.iter().map(|&x| trc.encode(x)).collect()
                    }
                };
                Image { width: d.width, height: d.height, data }
            }
        }
    }

    /// Replace the pixels with `img` (same size or 2x). The 8-bit alpha is kept (nearest at 2x).
    pub(super) fn put_image(p: Pixels, img: &Image) -> Pixels {
        match p {
            Pixels::Eight(old) => {
                let k = (img.width / old.width.max(1)).max(1);
                let mut out = Rgba8::new(img.width, img.height);
                for y in 0..img.height {
                    for x in 0..img.width {
                        let i = (y * img.width + x) * 3;
                        let c = |j: usize| (img.data[i + j].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        out.data[y * img.width + x] = [c(0), c(1), c(2), old.data[(y / k) * old.width + x / k][3]];
                    }
                }
                Pixels::Eight(out)
            }
            Pixels::Deep(d) => {
                let samples = match &d.samples {
                    DeepSamples::U16(_) => DeepSamples::U16(img.data.iter().map(|&v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).collect()),
                    DeepSamples::F32(_) => {
                        let trc = d.space.trc();
                        DeepSamples::F32(img.data.iter().map(|&v| trc.decode(v)).collect())
                    }
                };
                Pixels::Deep(DeepImage { width: img.width, height: img.height, space: d.space, samples })
            }
        }
    }

    /// Denoise `img`: SCUNet through an external PyTorch on the GPU when there is one (about 9x
    /// faster on a 22 Mpx photo, see [`crate::restore_torch`]), else the Rust implementation.
    fn run_denoise(img: &Image, progress: &mut dyn FnMut(usize, usize) -> bool) -> Result<Image, String> {
        use crate::restore_torch::{self as torch, Preference};
        let weights = super::models_dir().ok_or("no folder is set for the AI models")?.join(files::SCUNET.file.name);
        let pref = torch::preference();
        let runner = if pref == Preference::Rust { None } else { torch::detect() };
        match (runner, pref) {
            (Some(t), _) => match torch::denoise(t, img, &weights, progress) {
                Ok(d) => return Ok(d),
                Err(e) if e == "cancelled" || pref == Preference::Torch => return Err(format!("AI denoise (PyTorch): {e}")),
                Err(e) => log::warn!("AI denoise: the PyTorch runner failed, using the Rust implementation: {e}"),
            },
            (None, Preference::Torch) => {
                return Err("AI denoise: LIGHTCRAFT_RESTORE_BACKEND=torch but no Python with torch, numpy and MPS was found (LIGHTCRAFT_RESTORE_PYTHON names the interpreter)".into());
            }
            (None, _) => {}
        }
        load_denoiser()?.denoise(img, progress).map_err(|e| format!("AI denoise: {e}"))
    }

    /// Which backend denoising uses on this machine: `torch` (external PyTorch on MPS) or `rust`
    /// (candle). `probe` looks for PyTorch (a few seconds the first time); without it a backend
    /// not looked for yet is `unknown`.
    pub fn denoise_backend(probe: bool) -> Value {
        use crate::restore_torch::{self as torch, Preference};
        let pref = match torch::preference() {
            Preference::Auto => "auto",
            Preference::Rust => "rust",
            Preference::Torch => "torch",
        };
        let found = if probe && torch::preference() != Preference::Rust { torch::detect() } else { None };
        let (backend, python, version) = match found {
            Some(t) => ("torch", Some(t.python.display().to_string()), Some(t.version.clone())),
            None if probe || torch::preference() == Preference::Rust => ("rust", None, None),
            None => ("unknown", None, None),
        };
        json!({"preference": pref, "denoiseBackend": backend, "python": python, "torch": version})
    }

    pub fn apply(r: Rendered, req: Request) -> Result<Rendered, String> {
        // fail before the slow part when a model is missing
        if req.denoise > 0.0 && !installed(Model::Denoise) {
            return Err(missing(Model::Denoise));
        }
        let upscaler = if req.upscale { Some(load_upscaler()?) } else { None };
        let Rendered { image, deep, .. } = r;
        let mut pixels = match deep {
            Some(d) => Pixels::Deep(d),
            None => Pixels::Eight(image),
        };
        let progress = |what: &'static str| {
            move |done: usize, total: usize| {
                log::info!("AI {what}: tile {done}/{total}");
                true
            }
        };
        if req.denoise > 0.0 {
            let img = to_image(&pixels);
            let mut den = run_denoise(&img, &mut progress("denoise"))?;
            lightcraft_restore::blend(&img, &mut den, (req.denoise / 100.0) as f32).map_err(|e| e.to_string())?;
            pixels = put_image(pixels, &den);
        }
        if let Some(net) = upscaler {
            let img = to_image(&pixels);
            let up = net.upscale(&img, &mut progress("super-resolution")).map_err(|e| format!("super-resolution: {e}"))?;
            pixels = put_image(pixels, &up);
        }
        let (image, deep) = match pixels {
            Pixels::Eight(i) => (i, None),
            Pixels::Deep(d) => (d.to_rgba8(), Some(d)),
        };
        let histogram = Histogram::of_srgb8(&image);
        Ok(Rendered { image, histogram, deep })
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

    /// Environment variable with extra download locations (base URLs, comma/space separated),
    /// tried before the model's own.
    pub const MIRRORS_ENV: &str = "LIGHTCRAFT_RESTORE_MIRRORS";

    fn mirrors(m: Model) -> Vec<String> {
        let file = super::models_dir().map(|d| d.join("restore-mirrors.txt"));
        lightcraft_fetch::mirrors(std::env::var(MIRRORS_ENV).ok().as_deref(), file.as_deref(), &[m.spec().mirror])
    }

    /// Start downloading `m` in the background. `Ok(false)` when it is installed or downloading.
    pub fn start_download(m: Model) -> Result<bool, String> {
        let dir = super::models_dir().ok_or("no folder is set for the AI models")?;
        if installed(m) {
            return Ok(false);
        }
        let d = &DOWNLOADS[m.slot()];
        if d.running.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        d.cancel.store(false, Ordering::SeqCst);
        d.finished.store(false, Ordering::SeqCst);
        d.done.store(0, Ordering::SeqCst);
        d.total.store(m.spec().file.size.unwrap_or(0), Ordering::SeqCst);
        *lock(&d.error) = None;
        let mirrors = mirrors(m);
        let spec = m.spec();
        let spawned = std::thread::Builder::new().name(format!("restore-download-{}", m.id())).spawn(move || {
            let d = &DOWNLOADS[m.slot()];
            let file = [spec.file.clone()];
            let mut on_progress = |p: &lightcraft_fetch::Progress| {
                d.done.store(p.done, Ordering::SeqCst);
                d.total.store(p.total, Ordering::SeqCst);
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                lightcraft_fetch::download(&file, &mirrors, &dir, &lightcraft_fetch::Options::default(), &d.cancel, &mut on_progress)
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

    /// Stop a running download (its partial file is kept and resumed later).
    pub fn cancel_download(m: Model) -> bool {
        let d = &DOWNLOADS[m.slot()];
        let running = d.running.load(Ordering::SeqCst);
        if running {
            d.cancel.store(true, Ordering::SeqCst);
        }
        running
    }

    /// The facts about a model for `restore.model.status`.
    pub fn status(m: Model) -> Value {
        let (spec, d) = (m.spec(), &DOWNLOADS[m.slot()]);
        json!({
            "id": m.id(),
            "title": spec.title,
            "file": spec.file.name,
            "sizeBytes": spec.file.size,
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
        ALL.into_iter().map(status).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_comes_from_the_enhance_settings_and_nothing_is_a_noop() {
        let mut s = lightcraft_develop::DevelopSettings::default();
        assert!(Request::from_settings(&s).is_noop());
        s.enhance.restore_denoise = 250.0;
        s.enhance.super_resolution = true;
        let r = Request::from_settings(&s);
        assert_eq!(r, Request { denoise: 100.0, upscale: true });
        // a render passes through untouched when there is nothing to do
        let img = lightcraft_raster::Rgba8::from_fn(3, 2, |x, y| [x as u8, y as u8, 7, 255]);
        let hist = lightcraft_raster::Histogram::of_srgb8(&img);
        let out = apply(Rendered { image: img.clone(), histogram: hist, deep: None }, Request::default()).unwrap();
        assert_eq!(out.image, img);
    }

    #[test]
    fn the_denoise_control_is_a_0_to_100_slider_in_the_detail_section() {
        let spec = lightcraft_develop::controls::find("enhance.restoreDenoise").unwrap();
        assert_eq!((spec.min, spec.max, spec.default), (0.0, 100.0, 0.0));
        let mut s = lightcraft_develop::DevelopSettings::default();
        assert!(lightcraft_develop::controls::set(&mut s, "enhance.restoreDenoise", 140.0));
        assert_eq!(s.enhance.restore_denoise, 100.0);
    }

    #[cfg(feature = "restore")]
    #[test]
    fn pixels_round_trip_through_the_networks_domain_and_keep_alpha_when_doubled() {
        use super::imp::{Pixels, put_image, to_image};
        let img = lightcraft_raster::Rgba8::from_fn(5, 3, |x, y| [(x * 40) as u8, (y * 80) as u8, 200, if x == 0 { 0 } else { 255 }]);
        let f = to_image(&Pixels::Eight(img.clone()));
        let Pixels::Eight(back) = put_image(Pixels::Eight(img.clone()), &f) else { panic!("8-bit stays 8-bit") };
        assert_eq!(back, img);
        // doubled: nearest alpha, colour from the new image
        let big = lightcraft_restore::Image { width: 10, height: 6, data: vec![0.5; 10 * 6 * 3] };
        let Pixels::Eight(up) = put_image(Pixels::Eight(img), &big) else { panic!("8-bit stays 8-bit") };
        assert_eq!((up.width, up.height), (10, 6));
        assert_eq!(up.get(0, 0)[3], 0);
        assert_eq!(up.get(9, 5), [128, 128, 128, 255]);
        // 16-bit
        let d = lightcraft_pipeline::DeepImage {
            width: 2,
            height: 1,
            space: lightcraft_pipeline::OutputSpace::Srgb,
            samples: lightcraft_pipeline::DeepSamples::U16(vec![0, 65535, 32768, 1000, 2000, 3000]),
        };
        let f = to_image(&Pixels::Deep(d.clone()));
        let Pixels::Deep(back) = put_image(Pixels::Deep(d.clone()), &f) else { panic!("deep stays deep") };
        assert_eq!(back, d);
    }
}
