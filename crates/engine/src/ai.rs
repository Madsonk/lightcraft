//! The baked pixels the portrait sliders make ([`InpaintPatch`]): kept in memory and, with a library
//! on disk, in `<library>/inpaint/<id>.lcp`, and attached to the settings a render uses ([`Aux`]).
//! The settings only name them (a spot's `patch`), so rendering never needs a model.
//!
//! (Port note: the fork's `ai.rs` also held the depth maps of Lens Blur and the MI-GAN inpainting
//! model; neither is part of the 0.14.0 port, see `docs/port-014.md`.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightcraft_catalog::Photo;
use lightcraft_develop::{Aux, DevelopSettings, InpaintPatch};

use crate::{EngineError, Result, Session};

/// The patches in memory.
#[derive(Default)]
pub struct Ai {
    patches: HashMap<u64, Arc<InpaintPatch>>,
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

    pub(crate) fn store_patch(&mut self, p: InpaintPatch) -> Result<Arc<InpaintPatch>> {
        if let Some(file) = self.ai_file("inpaint", &format!("{}.lcp", p.hex_id())) {
            write_atomic(&file, &p.to_bytes()).map_err(|e| EngineError::Other(format!("{}: {e}", file.display())))?;
        }
        let p = Arc::new(p);
        self.ai.patches.insert(p.id, p.clone());
        Ok(p)
    }

    /// `s` with the render-time data its edits refer to attached (see [`Aux`]): the pixels of its
    /// portrait (inpaint) spots. Returns `s` itself when nothing is needed or nothing exists yet.
    pub(crate) fn attach_aux(&mut self, _p: &Photo, s: Arc<DevelopSettings>) -> Arc<DevelopSettings> {
        if !s.spots.iter().any(|sp| sp.patch.is_some()) {
            return s;
        }
        let patches: Vec<Arc<InpaintPatch>> = s.spots.iter().filter_map(|sp| sp.patch.clone()).filter_map(|hex| self.patch(&hex)).collect();
        if patches.is_empty() {
            return s;
        }
        let mut n = (*s).clone();
        n.aux = Aux { patches };
        Arc::new(n)
    }
}
