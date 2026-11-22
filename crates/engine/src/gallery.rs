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
    let ids: Vec<PhotoId> = s
        .targets(p)?
        .into_iter()
        .filter(|id| s.catalog.photo(*id).is_some_and(|p| matches!(p.kind, lightcraft_catalog::MediaKind::Image | lightcraft_catalog::MediaKind::Raw)))
        .collect();
    if ids.is_empty() {
        return Err(bad(C, "no photos to publish (select photos or pass ids)"));
    }
    let title = p.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).unwrap_or(&slug).to_string();
    let mut gallery = json!({"title": title, "download": p.get("download").and_then(Value::as_bool).unwrap_or(true)});
    if let Some(pw) = p.get("password") {
        gallery["password"] = pw.clone();
    }
    if let Some(days) = p.get("expiresInDays").and_then(Value::as_f64) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        gallery["expires"] = if days > 0.0 { json!(now + (days * 86_400.0) as u64) } else { Value::Null };
    }
    let edge = p.get("longEdge").and_then(Value::as_u64).unwrap_or(LONG_EDGE).clamp(480, 6000);
    let opts = ExportOptions::from_params(&json!({"format": "jpeg", "quality": 90, "resize": {"mode": "longEdge", "value": edge, "dontEnlarge": true}, "metadata": "copyright", "removeLocation": true}))
        .map_err(|e| bad(C, e.to_string()))?;
    let items = prepare_batch(s, &ids, &opts).map_err(|e| bad(C, e))?;
    let prints: Vec<String> = ids.iter().map(|id| fingerprint(s, *id, edge)).collect();
    let keys: Vec<(String, String)> = ids.iter().zip(&items).map(|(id, it)| (photo_key(&slug, *id), it.file_name.clone())).collect();

    let state = Arc::new(Mutex::new(State { phase: "running", slug: slug.clone(), total: items.len(), ..Default::default() }));
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = s.activity.start("gallery", "Publishing gallery", Cancel::Flag(cancel.clone()));
    guard.progress(0, items.len() as u64);
    s.gallery.job = Some((state.clone(), cancel.clone()));
    let (st, c) = (state.clone(), cancel.clone());
    let cfg2 = cfg.clone();
    let work = move || {
        let guard = guard;
        let run = || -> std::result::Result<(), String> {
            call(&cfg, "PUT", &format!("/api/v1/owner/galleries/{slug}"), gallery.to_string().as_bytes(), &[("Content-Type", "application/json".into())], &c)?;
            // pid → (sha, fingerprint) of what the server holds
            let known: std::collections::HashMap<String, (String, Option<String>)> = call(&cfg, "GET", &format!("/api/v1/owner/galleries/{slug}/manifest"), &[], &[], &c)?
                .get("photos")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|p| Some((p.get("pid")?.as_str()?.to_string(), (p.get("sha")?.as_str()?.to_string(), p.get("fp").and_then(Value::as_str).map(String::from))))).collect())
                .unwrap_or_default();
            let queue = Mutex::new(items.into_iter().zip(keys).zip(prints).enumerate());
            let (cfg, st, c, slug, known, guard) = (&cfg, &st, &c, &slug, &known, &guard);
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
                            if held.is_some_and(|(_, f)| f.as_deref() == Some(fp.as_str())) {
                                let mut g = lock(st);
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
                                if held.is_some_and(|(s, _)| *s == lightcraft_sha256_hex(&out.bytes)) {
                                    sent = Some(Ok(false));
                                } else {
                                    let h = [("Content-Type", "image/jpeg".into()), ("X-Position", pos.to_string()), ("X-Name", name.clone()), ("X-Fingerprint", fp.clone())];
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
            "{id?|ids?, slug?: a–z 0–9 - (random), expiresInDays?: n (0 = never), title?, password?: string (\"\" = none), download?: bool (true), longEdge?: px (2048), wait?: bool} — renders the photos at web size (no location) and sends them to the online gallery server (gallery.configure), only the ones the server does not hold yet; in the background with an activity row (gallery.status, gallery.cancel); wait blocks until done → status {state, slug, url, total, done, sent, unchanged, failed}",
            always,
            publish
        ),
        cmd!(query "gallery.list", "Online Galleries", [], None, "{} → the galleries on the server (slug, title, photos, expiry)", always, list),
        cmd!("gallery.delete", "Delete Online Gallery", [], None, "{slug} — deletes the gallery, its photos and likes from the server (irreversible; originals stay)", always, delete),
        cmd!("gallery.guests", "Gallery Guest Codes", [], None, "{slug, add?: name, level?: view|like|download (like), code?: string (random if omitted), revoke?: id} — no add/revoke lists the codes; an added code is shown once", always, guests),
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
    fn photo_keys_are_stable_per_gallery_and_differ_between_them() {
        let (a, b) = (photo_key("alpha-1", PhotoId(7)), photo_key("beta-22", PhotoId(7)));
        assert_eq!(a, photo_key("alpha-1", PhotoId(7)));
        assert_ne!(a, b);
        assert!(a.bytes().all(|c| c.is_ascii_alphanumeric()) && a.len() >= 3);
    }
}
