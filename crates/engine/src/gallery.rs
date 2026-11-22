//! Publishing photos to an online client gallery (`gallery.*`): the user's own gallery server
//! (see `5. Systeme/scripts/galeries-server` in the maintainer's vault; any server that speaks the
//! same `/api/v1/owner/…` calls works). Each photo is rendered with the normal export pipeline at web
//! size, location removed, then sent; a photo the server already holds with the same bytes is not
//! sent again, so publishing an album a second time only moves what changed.
//!
//! The address and the owner token come from `gallery.configure` (kept in `gallery.json` in the
//! settings folder), else from `$LIGHTCRAFT_GALLERY_URL` / `$LIGHTCRAFT_GALLERY_TOKEN`. The work runs
//! on a worker thread with a row in the activity stack and can be cancelled; `wait` blocks until
//! done (CLI, MCP, tests). Originals are only read.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use lightcraft_catalog::PhotoId;
use lightcraft_fetch::http::{Limits, Url, send};
use serde_json::{Value, json};

use crate::activity::Cancel;
use crate::cmd::{CommandSpec, always, bad, cmd};
use crate::export::{ExportOptions, prepare_batch};
use crate::{Result, Session};

const LONG_EDGE: u64 = 2048;
/// Photos rendered and sent at the same time.
const WORKERS: usize = 3;
const WAIT: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Config {
    url: String,
    token: String,
}

fn config_file() -> Option<std::path::PathBuf> {
    crate::config::config_dir().map(|d| d.join("gallery.json"))
}

/// The session's own setting first (`gallery.configure`), then the saved file, then the environment.
fn load_config(s: &Session) -> Config {
    if let Some(c) = s.gallery.config.clone().filter(|c| !c.url.is_empty() && !c.token.is_empty()) {
        return c;
    }
    disk_config()
}

fn disk_config() -> Config {
    let saved = config_file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str::<Config>(&t).ok());
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let mut c = saved.unwrap_or_default();
    if c.url.is_empty() {
        c.url = env("LIGHTCRAFT_GALLERY_URL").unwrap_or_default();
    }
    if c.token.is_empty() {
        c.token = env("LIGHTCRAFT_GALLERY_TOKEN").unwrap_or_default();
    }
    c
}

/// A photo's public id on the server: stable for a photo and a gallery, not guessable from the
/// catalogue's numbering (so a link never leaks how many photos the library holds).
pub fn photo_key(slug: &str, id: PhotoId) -> String {
    let salt = slug.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("p{:x}", lightcraft_catalog::query::mix64(id.0 ^ salt))
}

#[derive(Default)]
struct State {
    phase: &'static str, // idle | running | done | cancelled | failed
    slug: String,
    total: usize,
    done: usize,
    sent: usize,
    unchanged: usize,
    failed: Vec<Value>,
    error: Option<String>,
}

/// The publishing in progress or the last one.
#[derive(Default)]
pub struct Publisher {
    job: Option<(Arc<Mutex<State>>, Arc<AtomicBool>)>,
    config: Option<Config>,
}

fn lock(s: &Mutex<State>) -> MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn snapshot(st: &State, base: &str) -> Value {
    json!({
        "state": if st.phase.is_empty() { "idle" } else { st.phase }, "slug": st.slug,
        "url": if st.slug.is_empty() { Value::Null } else { json!(format!("{base}/g/{}", st.slug)) },
        "total": st.total, "done": st.done, "sent": st.sent, "unchanged": st.unchanged, "failed": st.failed, "error": st.error,
    })
}

fn call(cfg: &Config, method: &str, path: &str, body: &[u8], headers: &[(&str, String)], cancel: &AtomicBool) -> std::result::Result<Value, String> {
    let url = Url::parse(&format!("{}{path}", cfg.url.trim_end_matches('/'))).map_err(|e| e.to_string())?;
    let mut h = vec![("Authorization", format!("Bearer {}", cfg.token))];
    h.extend(headers.iter().map(|(k, v)| (*k, v.clone())));
    let limits = Limits { connect: Duration::from_secs(15), stall: Duration::from_secs(60), cancel };
    let (status, reply) = send(method, &url, &h, body, &limits).map_err(|e| e.to_string())?;
    let json: Value = serde_json::from_slice(&reply).unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        let why = json.get("detail").and_then(Value::as_str).unwrap_or("request refused");
        return Err(format!("{method} {path}: {status} {why}"));
    }
    Ok(json)
}

fn slug_for(p: &Value) -> std::result::Result<String, String> {
    match p.get("slug").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) if s.len() >= 3 && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !s.starts_with('-') => Ok(s.to_string()),
        Some(_) => Err("`slug` must be 3–64 characters: a–z, 0–9 and '-'".into()),
        None => {
            // no name given: a long random one, because a link without a password is the key
            use std::hash::{BuildHasher, Hasher};
            // `RandomState` is seeded by the operating system: two draws make 128 unguessable bits
            let draw = || std::collections::hash_map::RandomState::new().build_hasher().finish();
            Ok(format!("g-{:016x}{:016x}", draw(), draw()))
        }
    }
}

fn configure(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.configure";
    let mut c = load_config(s);
    if let Some(u) = p.get("url").and_then(Value::as_str) {
        let u = u.trim().trim_end_matches('/');
        if !u.is_empty() {
            Url::parse(u).map_err(|e| bad(C, e.to_string()))?;
            if !u.starts_with("https://") && !u.starts_with("http://127.0.0.1") && !u.starts_with("http://localhost") {
                return Err(bad(C, "the gallery address must be https:// (the owner token travels with every request)"));
            }
        }
        c.url = u.to_string();
    }
    if let Some(t) = p.get("token").and_then(Value::as_str) {
        c.token = t.trim().to_string();
    }
    if p.get("persist").and_then(Value::as_bool).unwrap_or(true) {
        let file = config_file().ok_or_else(|| bad(C, "no settings folder"))?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| bad(C, e.to_string()))?;
        }
        std::fs::write(&file, serde_json::to_vec_pretty(&c).unwrap_or_default()).map_err(|e| bad(C, e.to_string()))?;
    }
    s.gallery.config = Some(c.clone());
    Ok(json!({"url": c.url, "tokenSet": !c.token.is_empty()}))
}

fn config_info(s: &mut Session, _p: &Value) -> Result<Value> {
    let c = load_config(s);
    Ok(json!({"url": c.url, "tokenSet": !c.token.is_empty()}))
}

fn publish(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.publish";
    if s.gallery.job.as_ref().is_some_and(|(st, _)| lock(st).phase == "running") {
        return Err(bad(C, "a publication is already in progress (gallery.status, gallery.cancel)"));
    }
    let cfg = load_config(s);
    if cfg.url.is_empty() || cfg.token.is_empty() {
        return Err(bad(C, "no gallery server set: gallery.configure {url, token}"));
    }
    let slug = slug_for(p).map_err(|e| bad(C, e))?;
    let all: Vec<PhotoId> = match p.get("album").and_then(Value::as_u64) {
        Some(a) => album_photos(s, a).ok_or_else(|| bad(C, format!("no such album: {a}")))?,
        None => s.targets(p)?.into_iter().filter(|id| s.catalog.photo(*id).is_some()).collect(),
    };
    if all.is_empty() {
        return Err(bad(C, "no photos to publish (select photos or pass ids)"));
    }
    // (position in the selection, id): the gallery keeps the selection's order, videos among the photos
    let is_video = |id: &PhotoId| s.catalog.photo(*id).is_some_and(|p| p.kind == lightcraft_catalog::MediaKind::Video);
    let positions: Vec<usize> = (0..all.len()).filter(|i| !is_video(&all[*i])).collect();
    let ids: Vec<PhotoId> = positions.iter().map(|i| all[*i]).collect();
    let videos: Vec<(usize, PhotoId, String, String)> = (0..all.len())
        .filter(|i| is_video(&all[*i]))
        .filter_map(|i| {
            let ph = s.catalog.photo(all[i])?;
            let lightcraft_catalog::Source::File { path } = &ph.source else { return None };
            let fp = format!("v1video|{path}|{}", std::fs::metadata(path).map(|m| format!("{}-{:?}", m.len(), m.modified().ok())).unwrap_or_default());
            Some((i, all[i], path.clone(), lightcraft_sha256_hex(fp.as_bytes())))
        })
        .collect();
    let given = p.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
    // the website lists galleries by title: the random link is never shown in its place
    if given.is_none() && p.get("listed").and_then(Value::as_bool).unwrap_or(false) {
        return Err(bad(C, "a title is needed to show the gallery on the website"));
    }
    let title = given.unwrap_or(&slug).to_string();
    // adding to a gallery that exists: its title and download rule stay unless given
    let adding = p.get("append").and_then(Value::as_bool).unwrap_or(false);
    let mut gallery = json!({});
    if !adding || given.is_some() {
        gallery["title"] = json!(title);
    }
    if !adding || p.get("download").is_some() {
        gallery["download"] = json!(p.get("download").and_then(Value::as_bool).unwrap_or(true));
    }
    if let Some(pw) = p.get("password") {
        gallery["password"] = pw.clone();
    }
    if let Some(l) = p.get("listed").and_then(Value::as_bool) {
        gallery["listed"] = json!(l);
        // the public thumbnail is the first photo, only when asked for
        gallery["cover"] = if l && p.get("cover").and_then(Value::as_bool).unwrap_or(false) { json!(photo_key(&slug, all[0])) } else { Value::Null };
    }
    if let Some(days) = p.get("expiresInDays").and_then(Value::as_f64) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        gallery["expires"] = if days > 0.0 { json!(now + (days * 86_400.0) as u64) } else { Value::Null };
    }
    let edge = p.get("longEdge").and_then(Value::as_u64).unwrap_or(LONG_EDGE).clamp(480, 6000);
    let opts = ExportOptions::from_params(&json!({"format": "jpeg", "quality": 90, "resize": {"mode": "longEdge", "value": edge, "dontEnlarge": true}, "metadata": "copyright", "removeLocation": true}))
        .map_err(|e| bad(C, e.to_string()))?;
    let items = if ids.is_empty() { Vec::new() } else { prepare_batch(s, &ids, &opts).map_err(|e| bad(C, e))? };
    let prints: Vec<String> = ids.iter().map(|id| fingerprint(s, *id, edge)).collect();
    let keys: Vec<(String, String)> = ids.iter().zip(&items).map(|(id, it)| (photo_key(&slug, *id), it.file_name.clone())).collect();

    // chapter of each photo: the first regular album (by name) that lists it
    let chapters: std::collections::HashMap<String, String> = if p.get("chapters").and_then(Value::as_str) == Some("albums") {
        let mut albums: Vec<&lightcraft_catalog::Album> = s.catalog.albums().filter(|a| !a.folder && a.smart.is_none()).collect();
        albums.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        all.iter()
            .filter_map(|id| Some((photo_key(&slug, *id), albums.iter().find(|a| a.photos.contains(id))?.name.clone())))
            .collect()
    } else {
        Default::default()
    };
    let append = p.get("append").and_then(Value::as_bool).unwrap_or(false);
    let total = items.len() + videos.len();
    let state = Arc::new(Mutex::new(State { phase: "running", slug: slug.clone(), total, ..Default::default() }));
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = s.activity.start("gallery", "Publishing gallery", Cancel::Flag(cancel.clone()));
    guard.progress(0, total as u64);
    s.gallery.job = Some((state.clone(), cancel.clone()));
    let (st, c) = (state.clone(), cancel.clone());
    let cfg2 = cfg.clone();
    let work = move || {
        let guard = guard;
        let run = || -> std::result::Result<(), String> {
            call(&cfg, "PUT", &format!("/api/v1/owner/galleries/{slug}"), gallery.to_string().as_bytes(), &[("Content-Type", "application/json".into())], &c)?;
            // pid → (sha, fingerprint) of what the server holds
            let manifest = call(&cfg, "GET", &format!("/api/v1/owner/galleries/{slug}/manifest"), &[], &[], &c)?;
            // where new photos go when adding: after the last position online
            let next_pos = manifest["photos"].as_array().into_iter().flatten().filter_map(|p| p["pos"].as_i64()).max().map_or(0, |m| (m + 1).max(0) as usize);
            let known: std::collections::HashMap<String, (String, Option<String>, String)> = manifest
                .get("photos")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|p| Some((p.get("pid")?.as_str()?.to_string(), (p.get("sha")?.as_str()?.to_string(), p.get("fp").and_then(Value::as_str).map(String::from), p.get("section").and_then(Value::as_str).unwrap_or_default().to_string())))).collect())
                .unwrap_or_default();
            // adding to a gallery that already holds photos: they go after the ones online
            let off = if append { next_pos.max(known.len()) } else { 0 };
            let queue = Mutex::new(positions.iter().map(|p| p + off).zip(items.into_iter().zip(keys).zip(prints)));
            let (cfg, st, c, slug, known, guard, chapters) = (&cfg, &st, &c, &slug, &known, &guard, &chapters);
            // the photo is already online as it is: only its chapter may have moved
            let relabel = |pid: &str, pos: usize| -> std::result::Result<(), String> {
                let want = chapters.get(pid).map_or("", String::as_str);
                match known.get(pid) {
                    Some((_, _, have)) if have != want => call(cfg, "PATCH", &format!("/api/v1/owner/galleries/{slug}/photos/{pid}"), json!({"section": want, "position": pos}).to_string().as_bytes(), &[("Content-Type", "application/json".into())], c).map(|_| ()),
                    _ => Ok(()),
                }
            };
            // a few photos at once: one renders while another uploads
            std::thread::scope(|sc| {
                for _ in 0..WORKERS {
                    sc.spawn(|| {
                        loop {
                            if c.load(Ordering::Relaxed) {
                                return;
                            }
                            let Some((pos, ((item, (pid, name)), fp))) = queue.lock().unwrap_or_else(|e| e.into_inner()).next() else { return };
                            let held = known.get(&pid);
                            // same photo, same edits, same settings as what is online: nothing to render
                            if held.is_some_and(|(_, f, _)| f.as_deref() == Some(fp.as_str())) {
                                let r = relabel(&pid, pos);
                                let mut g = lock(st);
                                if let Err(e) = r {
                                    g.failed.push(json!([name, e]));
                                }
                                g.unchanged += 1;
                                g.done += 1;
                                guard.progress(g.done as u64, g.total as u64);
                                continue;
                            }
                            let out = match item.run() {
                                Ok(o) => Some(o),
                                Err(e) => {
                                    lock(st).failed.push(json!([name, e]));
                                    None
                                }
                            };
                            let mut sent = None;
                            if let Some(out) = &out {
                                if held.is_some_and(|(s, _, _)| *s == lightcraft_sha256_hex(&out.bytes)) {
                                    sent = Some(relabel(&pid, pos).map(|()| false));
                                } else {
                                    let h = [("Content-Type", "image/jpeg".into()), ("X-Position", pos.to_string()), ("X-Name", name.clone()), ("X-Fingerprint", fp.clone()), ("X-Section", pct_encode(chapters.get(&pid).map_or("", String::as_str)))];
                                    sent = Some(call(cfg, "PUT", &format!("/api/v1/owner/galleries/{slug}/photos/{pid}"), &out.bytes, &h, c).map(|_| true));
                                }
                            }
                            let mut g = lock(st);
                            match sent {
                                Some(Ok(true)) => g.sent += 1,
                                Some(Ok(false)) => g.unchanged += 1,
                                Some(Err(e)) => g.failed.push(json!([name, e])),
                                None => {}
                            }
                            g.done += 1;
                            guard.progress(g.done as u64, g.total as u64);
                        }
                    });
                }
            });
            for (pos, id, path, fp) in videos {
                let pos = pos + off;
                if c.load(Ordering::Relaxed) {
                    break;
                }
                let (pid, name) = (photo_key(slug, id), std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                let held = known.get(&pid);
                if held.is_some_and(|(_, f, _)| f.as_deref() == Some(fp.as_str())) {
                    let r = relabel(&pid, pos);
                    let mut g = lock(st);
                    if let Err(e) = r {
                        g.failed.push(json!([name, e]));
                    }
                    g.unchanged += 1;
                    g.done += 1;
                    guard.progress(g.done as u64, g.total as u64);
                    continue;
                }
                let r = send_video(cfg, slug, &pid, &path, pos, &name, &fp, &pct_encode(chapters.get(&pid).map_or("", String::as_str)), c);
                let mut g = lock(st);
                match r {
                    Ok(()) => g.sent += 1,
                    Err(e) => g.failed.push(json!([name, e])),
                }
                g.done += 1;
                guard.progress(g.done as u64, g.total as u64);
            }
            if c.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            Ok(())
        };
        let r = run();
        let mut g = lock(&st);
        match r {
            Ok(()) => g.phase = "done",
            Err(e) if e == "cancelled" => g.phase = "cancelled",
            Err(e) => {
                g.phase = "failed";
                g.error = Some(e);
            }
        }
    };
    let handle = std::thread::Builder::new().name("gallery-publish".into()).spawn(work).map_err(|e| bad(C, e.to_string()))?;
    if p.get("wait").and_then(Value::as_bool).unwrap_or(false) {
        let _ = WAIT;
        let _ = handle.join();
    }
    let base = cfg2.url.clone();
    Ok(snapshot(&lock(&state), base.trim_end_matches('/')))
}

/// Header-safe text: every byte outside `A-Za-z0-9-_.~` as `%XX` (UTF-8), what the server decodes.
fn pct_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Longest video a gallery takes (the web copy of a longer one would pass the server's size limit).
const VIDEO_MAX_SECONDS: f64 = 180.0;
const VIDEO_MAX_BYTES: u64 = 30 << 20;

/// A video's web copy (540 lines, H.264 + AAC, `+faststart`) and its poster frame, sent to the server:
/// the clip first (`X-Kind: video`), then the picture it shows before it plays.
#[allow(clippy::too_many_arguments)]
fn send_video(cfg: &Config, slug: &str, pid: &str, path: &str, pos: usize, name: &str, fp: &str, section: &str, cancel: &AtomicBool) -> std::result::Result<(), String> {
    use crate::video::ffmpeg as ff;
    let src = std::path::Path::new(path);
    let info = ff::probe(src)?;
    let secs = info.duration.unwrap_or(0.0);
    if secs > VIDEO_MAX_SECONDS {
        return Err(format!("{secs:.0} s is longer than the {VIDEO_MAX_SECONDS:.0} s a gallery takes: trim it first"));
    }
    let dir = std::env::temp_dir().join(format!("lightcraft-gallery-{pid}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (clip, poster) = (dir.join("clip.mp4"), dir.join("poster.jpg"));
    let result = (|| {
        ff::extract_clip(src, 0, (secs.max(1.0) * 1000.0) as u64, &clip)?;
        ff::extract_frame(src, ((secs * 0.1) * 1000.0) as u64, 1600, &poster)?;
        let (video, still) = (std::fs::read(&clip).map_err(|e| e.to_string())?, std::fs::read(&poster).map_err(|e| e.to_string())?);
        if video.len() as u64 > VIDEO_MAX_BYTES {
            return Err(format!("the web copy is {} MB, over the {} MB limit: trim it first", video.len() >> 20, VIDEO_MAX_BYTES >> 20));
        }
        let h = [("Content-Type", "video/mp4".into()), ("X-Kind", "video".into()), ("X-Position", pos.to_string()), ("X-Name", name.to_string()), ("X-Fingerprint", fp.to_string()), ("X-Section", section.to_string())];
        call(cfg, "PUT", &format!("/api/v1/owner/galleries/{slug}/photos/{pid}"), &video, &h, cancel)?;
        call(cfg, "PUT", &format!("/api/v1/owner/galleries/{slug}/photos/{pid}/poster"), &still, &[("Content-Type", "image/jpeg".into())], cancel)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// What a published photo depends on: the file, its edits and the web settings. The photographer's
/// keywords are left out (a like adds one) so bringing likes home never triggers a re-render.
fn fingerprint(s: &Session, id: PhotoId, edge: u64) -> String {
    let Some(ph) = s.catalog.photo(id) else { return String::new() };
    let mut meta = ph.meta.clone();
    meta.keywords.clear();
    let text = format!("v1|{edge}|{}|{}|{}|{}", ph.content_hash.as_deref().unwrap_or(""), id.0, serde_json::to_string(&*ph.develop).unwrap_or_default(), serde_json::to_string(&meta).unwrap_or_default());
    lightcraft_sha256_hex(text.as_bytes())
}

/// The SHA-256 the server computes for the same bytes.
fn lightcraft_sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Keyword that marks a photo a client liked in an online gallery.
const LIKED: &str = "Client favourite";

/// Brings the clients' likes home: every photo with at least one like gets the keyword `LIKED`, a photo
/// whose likes were all taken back loses it. Flags, ratings and labels are the photographer's own and
/// are left alone. Runs in the foreground (one small request); safe to repeat.
fn pull(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.pull";
    let cfg = load_config(s);
    if cfg.url.is_empty() || cfg.token.is_empty() {
        return Err(bad(C, "no gallery server set: gallery.configure {url, token}"));
    }
    let only = p.get("slug").and_then(Value::as_str);
    let reply = call(&cfg, "GET", "/api/v1/owner/likes?since=0", &[], &[], &AtomicBool::new(false)).map_err(|e| bad(C, e))?;
    // (slug, pid) → does anybody still like it
    let mut liked: std::collections::HashMap<(String, String), bool> = Default::default();
    for c in reply.get("changes").and_then(Value::as_array).into_iter().flatten() {
        let (Some(slug), Some(pid)) = (c.get("slug").and_then(Value::as_str), c.get("pid").and_then(Value::as_str)) else { continue };
        if only.is_some_and(|o| o != slug) {
            continue;
        }
        let on = c.get("removed").and_then(Value::as_i64).unwrap_or(0) == 0;
        *liked.entry((slug.to_string(), pid.to_string())).or_default() |= on;
    }
    let slugs: std::collections::HashSet<&str> = liked.keys().map(|(s, _)| s.as_str()).collect();
    let (mut add, mut remove, mut found) = (Vec::new(), Vec::new(), 0usize);
    for ph in s.catalog.photos() {
        for slug in &slugs {
            if let Some(&on) = liked.get(&((*slug).to_string(), photo_key(slug, ph.id))) {
                found += 1;
                if on { add.push(ph.id.0) } else { remove.push(ph.id.0) }
            }
        }
    }
    if !add.is_empty() {
        s.execute("photo.setMeta", &json!({"ids": add, "addKeywords": [LIKED]}))?;
    }
    if !remove.is_empty() {
        s.execute("photo.setMeta", &json!({"ids": remove, "removeKeywords": [LIKED]}))?;
    }
    Ok(json!({"liked": add.len(), "unliked": remove.len(), "notInLibrary": liked.len() - found, "keyword": LIKED}))
}

fn owner_call(s: &Session, c: &str, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let cfg = load_config(s);
    if cfg.url.is_empty() || cfg.token.is_empty() {
        return Err(bad(c, "no gallery server set: gallery.configure {url, token}"));
    }
    let (bytes, h) = match body {
        Some(b) => (b.to_string().into_bytes(), vec![("Content-Type", "application/json".to_string())]),
        None => (Vec::new(), Vec::new()),
    };
    call(&cfg, method, path, &bytes, &h, &AtomicBool::new(false)).map_err(|e| bad(c, e))
}

fn slug_param(p: &Value, c: &str) -> Result<String> {
    slug_for(p).ok().filter(|_| p.get("slug").is_some()).ok_or_else(|| bad(c, "`slug` is required (a–z 0–9 -)"))
}

fn list(s: &mut Session, _p: &Value) -> Result<Value> {
    owner_call(s, "gallery.list", "GET", "/api/v1/owner/galleries", None)
}

/// Takes a gallery offline for good: its photos and likes on the server are deleted (the originals stay).
fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let slug = slug_param(p, "gallery.delete")?;
    owner_call(s, "gallery.delete", "DELETE", &format!("/api/v1/owner/galleries/{slug}"), None)
}

fn guests(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.guests";
    let slug = slug_param(p, C)?;
    let base = format!("/api/v1/owner/galleries/{slug}/guests");
    if let Some(id) = p.get("revoke").and_then(Value::as_i64) {
        return owner_call(s, C, "DELETE", &format!("{base}/{id}"), None);
    }
    if let Some(name) = p.get("add").and_then(Value::as_str) {
        let mut b = json!({"name": name, "level": p.get("level").and_then(Value::as_str).unwrap_or("like")});
        if let Some(code) = p.get("code") {
            b["code"] = code.clone();
        }
        return owner_call(s, C, "POST", &base, Some(b));
    }
    owner_call(s, C, "GET", &base, None)
}

/// The access requests of a gallery, and the owner's decision on one: approving makes the server email the
/// visitor their personal link.
fn requests(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.requests";
    let slug = slug_param(p, C)?;
    let base = format!("/api/v1/owner/galleries/{slug}/requests");
    if let Some(id) = p.get("decide").and_then(Value::as_i64) {
        let decision = p.get("decision").and_then(Value::as_str).filter(|d| matches!(*d, "approve" | "deny")).ok_or_else(|| bad(C, "`decision` is approve or deny"))?;
        let level = p.get("level").and_then(Value::as_str).filter(|l| matches!(*l, "like" | "download")).unwrap_or("like");
        return owner_call(s, C, "POST", &format!("{base}/{id}"), Some(json!({"decision": decision, "level": level})));
    }
    owner_call(s, C, "GET", &base, None)
}

/// The photos an album lists (a regular album's, or the ones a smart album's rules match).
fn album_photos(s: &Session, album: u64) -> Option<Vec<PhotoId>> {
    let id = lightcraft_catalog::AlbumId(album);
    let a = s.catalog.albums().find(|a| a.id == id)?;
    if a.folder {
        return None;
    }
    Some(if a.smart.is_none() {
        a.photos.iter().copied().filter(|p| s.catalog.photo(*p).is_some()).collect()
    } else {
        s.catalog.photos().filter(|p| p.in_library() && s.catalog.album_contains(id, p)).map(|p| p.id).collect()
    })
}

/// The server's photo key → the library photo it was made from (the same key `publish` derives).
fn key_map(s: &Session, slug: &str) -> std::collections::HashMap<String, PhotoId> {
    s.catalog.photos().map(|p| (photo_key(slug, p.id), p.id)).collect()
}

fn gallery_row(s: &Session, c: &str, slug: &str) -> Result<Value> {
    owner_call(s, c, "GET", "/api/v1/owner/galleries", None)?
        .get("galleries")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|g| g["slug"] == slug).cloned())
        .ok_or_else(|| bad(c, format!("no gallery `{slug}` on the server")))
}

/// A gallery as the server holds it: its settings and its photos in display order, each with the library
/// photo it comes from (`id`, null when it was published from another library).
fn open(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.open";
    let slug = slug_param(p, C)?;
    let gallery = gallery_row(s, C, &slug)?;
    let manifest = owner_call(s, C, "GET", &format!("/api/v1/owner/galleries/{slug}/manifest"), None)?;
    let map = key_map(s, &slug);
    let photos: Vec<Value> = manifest["photos"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let pid = m["pid"].as_str().unwrap_or_default();
            json!({"pid": pid, "id": map.get(pid).map(|i| i.0), "name": m["name"], "section": m["section"], "position": m["pos"], "kind": m["kind"], "hidden": m["hidden"]})
        })
        .collect();
    Ok(json!({"gallery": gallery, "photos": photos}))
}

/// Changes a gallery's settings without sending a photo. Only the keys given change.
fn update(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.update";
    let slug = slug_param(p, C)?;
    let row = gallery_row(s, C, &slug)?;
    // the server resets what a PUT leaves out on older versions: the current title and download rule always go along
    let mut body = json!({"title": p.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).or(row["title"].as_str()).unwrap_or(&slug), "download": p.get("download").and_then(Value::as_bool).unwrap_or_else(|| row["download"].as_bool().or_else(|| row["download"].as_i64().map(|n| n != 0)).unwrap_or(true))});
    for k in ["password", "listed", "sections_order"] {
        if let Some(v) = p.get(k) {
            body[k] = v.clone();
        }
    }
    if let Some(days) = p.get("expiresInDays").and_then(Value::as_f64) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        body["expires"] = if days > 0.0 { json!(now + (days * 86_400.0) as u64) } else { Value::Null };
    }
    if let Some(c) = p.get("cover") {
        // a library photo id (or null to remove the cover)
        body["cover"] = c.as_u64().map_or(Value::Null, |id| json!(photo_key(&slug, PhotoId(id))));
    }
    owner_call(s, C, "PUT", &format!("/api/v1/owner/galleries/{slug}"), Some(body))
}

/// Takes photos out of a gallery (`hide: true` keeps them on the server but out of the page).
fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.remove";
    let slug = slug_param(p, C)?;
    let ids: Vec<u64> = p.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    if ids.is_empty() {
        return Err(bad(C, "`ids`: the library photos to take out"));
    }
    let hide = p.get("hide").and_then(Value::as_bool).unwrap_or(false);
    let mut done = 0;
    for id in ids {
        let pid = photo_key(&slug, PhotoId(id));
        if hide {
            owner_call(s, C, "POST", &format!("/api/v1/owner/galleries/{slug}/hide"), Some(json!({"pid": pid, "hidden": p.get("hidden").and_then(Value::as_bool).unwrap_or(true)})))?;
        } else {
            owner_call(s, C, "DELETE", &format!("/api/v1/owner/galleries/{slug}/photos/{pid}"), None)?;
        }
        done += 1;
    }
    Ok(json!({"removed": done, "hidden": hide}))
}

/// Puts a gallery's photos in order and in chapters: `ids` in display order, `sections` = {"<id>": chapter}.
fn arrange(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.arrange";
    let slug = slug_param(p, C)?;
    let ids: Vec<u64> = p.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    if ids.is_empty() {
        return Err(bad(C, "`ids`: the library photos in display order"));
    }
    let empty = json!({});
    let sections = p.get("sections").unwrap_or(&empty);
    let mut done = 0;
    for (pos, id) in ids.iter().enumerate() {
        let mut body = json!({"position": pos});
        if let Some(name) = sections.get(id.to_string()).and_then(Value::as_str) {
            body["section"] = json!(name);
        }
        owner_call(s, C, "PATCH", &format!("/api/v1/owner/galleries/{slug}/photos/{}", photo_key(&slug, PhotoId(*id))), Some(body))?;
        done += 1;
    }
    if let Some(order) = p.get("chapters") {
        update(s, &json!({"slug": slug, "sections_order": order}))?;
    }
    Ok(json!({"arranged": done}))
}

/// "YYYY-MM-DDTHH:MM:SS…" → seconds since 1970 (the clock as written, no time zone: only differences matter).
fn capture_seconds(s: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13).unwrap_or(0), n(14..16).unwrap_or(0), n(17..19).unwrap_or(0));
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// What the library knows about each photo, as the organiser reads it.
fn candidates(s: &Session, ids: &[PhotoId]) -> Vec<crate::gallery_ai::Candidate> {
    let mut manual: Vec<&lightcraft_catalog::Album> = s.catalog.albums().filter(|a| !a.folder && a.smart.is_none()).collect();
    manual.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    ids.iter()
        .filter_map(|id| {
            let ph = s.catalog.photo(*id)?;
            let an = ph.analysis.as_ref();
            Some(crate::gallery_ai::Candidate {
                id: *id,
                captured: ph.captured.as_deref().and_then(capture_seconds),
                sharpness: an.map(|a| a.sharpness),
                verdict_score: an.and_then(|a| a.verdict.as_ref()).map(|v| v.score),
                retouch_score: an.and_then(|a| a.retouch.as_ref()).map(|r| r.score),
                burst: an.and_then(|a| a.group),
                best_of_burst: an.is_some_and(|a| a.best),
                faces: an.and_then(|a| a.faces.as_ref()).map(|f| f.len() as u32),
                people: Vec::new(),
                // place read from the IPTC fields: a chapter shared by most photos takes its name
                scene: Some(ph.meta.location.trim()).filter(|l| !l.is_empty()).or_else(|| Some(ph.meta.city.trim()).filter(|c| !c.is_empty())).map(str::to_owned),
                width: ph.width,
                height: ph.height,
                rating: ph.rating,
                flag_pick: ph.flag == lightcraft_catalog::Flag::Pick,
                flag_reject: ph.flag == lightcraft_catalog::Flag::Reject,
                album: manual.iter().find(|a| a.photos.contains(id)).map(|a| a.name.clone()),
            })
        })
        .collect()
}

/// Proposes how to organise a gallery — chapters, order, a cover, the photos to leave out — from what
/// LightCraft knows about the photos (sharpness, bursts, faces, capture time…). Nothing leaves the machine
/// and nothing changes: `gallery.applyPlan` applies a plan.
fn organise(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.organise";
    let ids: Vec<PhotoId> = if let Some(slug) = p.get("slug").and_then(Value::as_str) {
        let manifest = owner_call(s, C, "GET", &format!("/api/v1/owner/galleries/{slug}/manifest"), None)?;
        let map = key_map(s, slug);
        manifest["photos"].as_array().into_iter().flatten().filter_map(|m| map.get(m["pid"].as_str()?).copied()).collect()
    } else {
        s.targets(p)?
    };
    if ids.is_empty() {
        return Err(bad(C, "no photos to organise (`slug` of a gallery, or select photos)"));
    }
    let opts = crate::gallery_ai::Options {
        lang: if p.get("lang").and_then(Value::as_str) == Some("en") { crate::gallery_ai::Lang::En } else { crate::gallery_ai::Lang::Fr },
        gap_minutes: p.get("gapMinutes").and_then(Value::as_i64).unwrap_or(45).clamp(5, 24 * 60),
        drop_weak: p.get("dropWeak").and_then(Value::as_bool).unwrap_or(true),
        target_cover_landscape: p.get("landscapeCover").and_then(Value::as_bool).unwrap_or(false),
    };
    let plan = crate::gallery_ai::organise(&candidates(s, &ids), &opts);
    Ok(json!({
        "chapters": plan.chapters.iter().map(|c| json!({"name": c.name, "ids": c.ids.iter().map(|i| i.0).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "cover": plan.cover.map(|i| i.0),
        "dropped": plan.dropped.iter().map(|d| json!({"id": d.id.0, "reason": d.reason})).collect::<Vec<_>>(),
        "notes": plan.notes,
    }))
}

/// Applies a plan from `gallery.organise`: the order and chapters, the cover, and the dropped photos hidden
/// (not deleted: they can be shown again).
fn apply_plan(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "gallery.applyPlan";
    let slug = slug_param(p, C)?;
    let plan = p.get("plan").ok_or_else(|| bad(C, "`plan`: the result of gallery.organise"))?;
    let (mut ids, mut sections, mut names) = (Vec::new(), serde_json::Map::new(), Vec::new());
    for ch in plan["chapters"].as_array().into_iter().flatten() {
        let name = ch["name"].as_str().unwrap_or_default().to_string();
        for id in ch["ids"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
            ids.push(id);
            sections.insert(id.to_string(), json!(name));
        }
        names.push(json!(name));
    }
    let dropped: Vec<u64> = plan["dropped"].as_array().into_iter().flatten().filter_map(|d| d["id"].as_u64()).collect();
    if !ids.is_empty() {
        arrange(s, &json!({"slug": slug, "ids": ids, "sections": sections, "chapters": names}))?;
        // what the plan keeps is shown, in case an earlier plan had hidden it
        remove(s, &json!({"slug": slug, "ids": ids, "hide": true, "hidden": false}))?;
    }
    if !dropped.is_empty() {
        remove(s, &json!({"slug": slug, "ids": dropped, "hide": true}))?;
    }
    if let Some(cover) = plan["cover"].as_u64() {
        update(s, &json!({"slug": slug, "cover": cover}))?;
    }
    Ok(json!({"arranged": ids.len(), "hidden": dropped.len(), "cover": plan["cover"]}))
}

fn status(s: &mut Session, _p: &Value) -> Result<Value> {
    let base = load_config(s).url;
    Ok(match &s.gallery.job {
        Some((st, _)) => snapshot(&lock(st), base.trim_end_matches('/')),
        None => snapshot(&State::default(), base.trim_end_matches('/')),
    })
}

fn cancel(s: &mut Session, _p: &Value) -> Result<Value> {
    if let Some((_, c)) = &s.gallery.job {
        c.store(true, Ordering::Relaxed);
    }
    Ok(json!({"cancelling": s.gallery.job.is_some()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "gallery.publish",
            "Publish to Online Gallery",
            [],
            None,
            "{id?|ids?, slug?: a–z 0–9 - (random), album?: album id (publish that album's photos), append?: bool (add after the photos already online), chapters?: \"albums\" (each photo's first regular album, by name, becomes its chapter), expiresInDays?: n (0 = never), listed?: bool (show in the website menu), cover?: bool (first photo as public thumbnail), title?, password?: string (\"\" = none), download?: bool (true), longEdge?: px (2048), wait?: bool} — renders the photos at web size (no location) and sends them to the online gallery server (gallery.configure), only the ones the server does not hold yet; in the background with an activity row (gallery.status, gallery.cancel); wait blocks until done → status {state, slug, url, total, done, sent, unchanged, failed}",
            always,
            publish
        ),
        cmd!(query "gallery.list", "Online Galleries", [], None, "{} → the galleries on the server (slug, title, photos, expiry)", always, list),
        cmd!("gallery.delete", "Delete Online Gallery", [], None, "{slug} — deletes the gallery, its photos and likes from the server (irreversible; originals stay)", always, delete),
        cmd!("gallery.guests", "Gallery Guest Codes", [], None, "{slug, add?: name, level?: view|like|download (like), code?: string (random if omitted), revoke?: id} — no add/revoke lists the codes; an added code is shown once", always, guests),
        cmd!("gallery.requests", "Gallery Access Requests", [], None, "{slug, decide?: request id, decision?: approve|deny, level?: like|download (like)} — no decide lists the requests (id, name, email, message, status); approving emails the visitor their personal link", always, requests),
        cmd!(query "gallery.open", "Open Online Gallery", [], None, "{slug} → {gallery: settings, photos: [{pid, id (library photo or null), name, section, position, kind, hidden}]} in display order", always, open),
        cmd!("gallery.update", "Edit Online Gallery", [], None, "{slug, title?, password?: string (\"\" removes it), expiresInDays?: n (0 = never), listed?: bool, cover?: library photo id (null removes), download?: bool, sections_order?: [chapter names]} — only the keys given change", always, update),
        cmd!("gallery.remove", "Remove From Online Gallery", [], None, "{slug, ids: library photo ids, hide?: bool (keep on the server, out of the page), hidden?: bool (true; false shows them again)} — deletes the web copies from the server; the originals are untouched", always, remove),
        cmd!("gallery.arrange", "Arrange Online Gallery", [], None, "{slug, ids: library photo ids in display order, sections?: {\"<id>\": chapter name}, chapters?: [chapter order]} — order and chapters only, nothing is sent again", always, arrange),
        cmd!(query "gallery.organise", "Organise Gallery With AI", [], None, "{slug? | ids?, lang?: fr|en, dropWeak?: bool (true), gapMinutes?: n (45), landscapeCover?: bool} → {chapters: [{name, ids}], cover, dropped: [{id, reason}], notes} — a proposal made on this machine from sharpness, bursts, faces and capture time; nothing is sent or changed", always, organise),
        cmd!("gallery.applyPlan", "Apply Gallery Plan", [], None, "{slug, plan: gallery.organise's result} — order and chapters, the cover, the dropped photos hidden (not deleted)", always, apply_plan),
        cmd!(query "gallery.status", "Online Gallery progress", [], None, "{} → {state: idle|running|done|cancelled|failed, slug, url, total, done, sent, unchanged, failed, error}", always, status),
        cmd!("gallery.pull", "Bring Gallery Likes Home", [], None, "{slug?} → {liked, unliked, notInLibrary, keyword} — reads the clients' likes from the online gallery server and gives each liked photo the keyword \"Client favourite\" (removed again when every like is taken back); flags, ratings and labels are not touched", always, pull),
        cmd!(query "gallery.cancel", "Cancel Gallery Publishing", [], None, "{} — stops between two photos; what was sent stays online", always, cancel),
        cmd!(query "gallery.configure", "Online Gallery Server", [], None, "{url?: https address, token?: owner token, persist?: bool (true)} — saved in the settings folder (gallery.json), else $LIGHTCRAFT_GALLERY_URL / _TOKEN → {url, tokenSet}", always, configure),
        cmd!(query "gallery.settings", "Online Gallery settings", [], None, "{} → {url, tokenSet}", always, config_info),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{Read, Write};

    /// A stand-in for the gallery server: keeps the photos it is sent, answers the manifest.
    fn server(listener: std::net::TcpListener, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let (mut held, mut log): (HashMap<String, (String, String)>, Vec<String>) = Default::default();
            listener.set_nonblocking(true).ok();
            while !stop.load(Ordering::Relaxed) {
                let Ok((mut s, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                s.set_nonblocking(false).ok();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 65536];
                let (head_end, len) = loop {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                        let len = head.lines().find_map(|l| l.strip_prefix("content-length: ")?.trim().parse::<usize>().ok()).unwrap_or(0);
                        if buf.len() >= i + 4 + len {
                            break (i + 4, len);
                        }
                    }
                    if n == 0 {
                        break (buf.len(), 0);
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                let line = head.lines().next().unwrap_or_default().to_string();
                let body = &buf[head_end..head_end + len];
                assert!(head.contains("Authorization: Bearer tok"), "{head}");
                let reply = if line.contains("/manifest") {
                    json!({"photos": held.iter().map(|(k, v)| json!({"pid": k, "sha": v.0, "fp": v.1})).collect::<Vec<_>>()}).to_string()
                } else {
                    if let Some(pid) = line.split("/photos/").nth(1).and_then(|r| r.split(' ').next()) {
                        let fp = head.lines().find_map(|l| l.strip_prefix("X-Fingerprint: ")).unwrap_or_default().to_string();
                        held.insert(pid.to_string(), (lightcraft_sha256_hex(body), fp));
                    }
                    "{}".into()
                };
                log.push(line);
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes());
            }
            log
        })
    }

    /// Publishing twice sends each photo once; the second run finds them all unchanged. A refused address
    /// and a missing server are refused before anything is rendered.
    #[test]
    fn publishing_sends_only_what_the_server_does_not_hold() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let stop = Arc::new(AtomicBool::new(false));
        let srv = server(listener, stop.clone());
        let mut s = Session::with_demo();
        assert!(s.execute("gallery.configure", &json!({"url": "http://example.com", "token": "tok", "persist": false})).is_err(), "plain http to a remote host");
        s.execute("gallery.configure", &json!({"url": format!("http://127.0.0.1:{port}"), "token": "tok", "persist": false})).expect("configure");
        let ids: Vec<u64> = s.visible_cloned().iter().take(2).map(|i| i.0).collect();
        let first = s.execute("gallery.publish", &json!({"ids": ids, "slug": "essai-moteur", "title": "Essai", "wait": true})).expect("publish");
        assert_eq!((first["state"].as_str(), first["sent"].as_u64(), first["unchanged"].as_u64()), (Some("done"), Some(2), Some(0)), "{first}");
        let again = s.execute("gallery.publish", &json!({"ids": ids, "slug": "essai-moteur", "wait": true})).expect("publish again");
        assert_eq!((again["state"].as_str(), again["sent"].as_u64(), again["unchanged"].as_u64()), (Some("done"), Some(0), Some(2)), "{again}");
        assert_eq!(again["url"], format!("http://127.0.0.1:{port}/g/essai-moteur"));
        stop.store(true, Ordering::Relaxed);
        let log = srv.join().expect("server");
        assert_eq!(log.iter().filter(|l| l.starts_with("PUT") && l.contains("/photos/")).count(), 2, "{log:?}");
    }

    /// Likes come back as a keyword on exactly the photos liked; a photo whose likes were all withdrawn loses it.
    #[test]
    fn likes_come_home_as_a_keyword() {
        let mut s = Session::with_demo();
        let ids: Vec<PhotoId> = s.visible_cloned().iter().take(3).copied().collect();
        let k = |i: usize| photo_key("g-test", ids[i]);
        let body = json!({"changes": [
            {"slug": "g-test", "pid": k(0), "who": "a", "removed": 0, "seq": 1},
            {"slug": "g-test", "pid": k(1), "who": "a", "removed": 1, "seq": 3},
            {"slug": "g-test", "pid": "pdead", "who": "a", "removed": 0, "seq": 4}], "cursor": 4})
        .to_string();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let srv = std::thread::spawn(move || {
            let (mut c, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 4096];
            let _ = c.read(&mut buf);
            let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        });
        s.execute("gallery.configure", &json!({"url": format!("http://127.0.0.1:{port}"), "token": "tok", "persist": false})).expect("configure");
        s.execute("photo.setMeta", &json!({"ids": [ids[1].0], "addKeywords": [LIKED]})).expect("pre-existing");
        let r = s.execute("gallery.pull", &json!({})).expect("pull");
        srv.join().expect("server");
        assert_eq!((r["liked"].as_u64(), r["unliked"].as_u64(), r["notInLibrary"].as_u64()), (Some(1), Some(1), Some(1)), "{r}");
        let has = |i: usize| s.catalog.photo(ids[i]).is_some_and(|p| p.meta.keywords.iter().any(|x| x == LIKED));
        assert!(has(0) && !has(1) && !has(2));
    }

    #[test]
    fn a_request_decision_is_checked_before_anything_is_sent() {
        let mut s = Session::with_demo();
        s.execute("gallery.configure", &json!({"url": "http://127.0.0.1:9", "token": "tok", "persist": false})).expect("configure");
        assert!(s.execute("gallery.requests", &json!({"decide": 1, "decision": "approve"})).is_err(), "a slug is required");
        let e = s.execute("gallery.requests", &json!({"slug": "essai-1", "decide": 1, "decision": "maybe"})).expect_err("bad decision").to_string();
        assert!(e.contains("approve or deny"), "{e}");
    }

    #[test]
    fn a_listed_gallery_needs_a_title() {
        let mut s = Session::with_demo();
        s.execute("gallery.configure", &json!({"url": "http://127.0.0.1:9", "token": "tok", "persist": false})).expect("configure");
        let ids: Vec<u64> = s.visible_cloned().iter().take(1).map(|i| i.0).collect();
        let e = s.execute("gallery.publish", &json!({"ids": ids, "listed": true})).expect_err("no title").to_string();
        assert!(e.contains("title is needed"), "{e}");
    }

    #[test]
    fn capture_times_read_as_seconds() {
        assert_eq!(capture_seconds("1970-01-02T00:00:00"), Some(86_400));
        assert_eq!(capture_seconds("2026-01-17T12:39:04.810").map(|s| s % 86_400), Some(12 * 3600 + 39 * 60 + 4));
        assert_eq!(capture_seconds("2026-03-01T00:00:00").zip(capture_seconds("2026-02-28T00:00:00")).map(|(a, b)| a - b), Some(86_400)); // 2026 is no leap year
        assert_eq!(capture_seconds("garbage"), None);
    }

    #[test]
    fn management_commands_need_a_gallery() {
        let mut s = Session::with_demo();
        s.execute("gallery.configure", &json!({"url": "http://127.0.0.1:9", "token": "tok", "persist": false})).expect("configure");
        for c in ["gallery.open", "gallery.update", "gallery.remove", "gallery.arrange", "gallery.applyPlan"] {
            assert!(s.execute(c, &json!({})).is_err(), "{c} without a slug");
        }
        assert!(s.execute("gallery.remove", &json!({"slug": "essai-1", "ids": []})).is_err(), "nothing to remove");
        let ids: Vec<u64> = s.visible_cloned().iter().take(3).map(|i| i.0).collect();
        let plan = s.execute("gallery.organise", &json!({"ids": ids, "lang": "en"})).expect("organise needs no server");
        let kept: usize = plan["chapters"].as_array().map_or(0, |c| c.iter().map(|c| c["ids"].as_array().map_or(0, Vec::len)).sum());
        assert_eq!(kept + plan["dropped"].as_array().map_or(0, Vec::len), 3, "{plan}");
    }

    #[test]
    fn chapter_names_travel_as_ascii_headers() {
        assert_eq!(pct_encode("Cérémonie 1"), "C%C3%A9r%C3%A9monie%201");
        assert_eq!(pct_encode("abc-_.~"), "abc-_.~");
        assert_eq!(pct_encode(""), "");
    }

    #[test]
    fn photo_keys_are_stable_per_gallery_and_differ_between_them() {
        let (a, b) = (photo_key("alpha-1", PhotoId(7)), photo_key("beta-22", PhotoId(7)));
        assert_eq!(a, photo_key("alpha-1", PhotoId(7)));
        assert_ne!(a, b);
        assert!(a.bytes().all(|c| c.is_ascii_alphanumeric()) && a.len() >= 3);
    }
}
