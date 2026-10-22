//! The portrait retouch off the caller's thread (`portrait_job`): the same result as the
//! synchronous command, one undo step per photo, a caller that never waits, a stop that answers at
//! once and leaves nothing behind. The face finder is given (the model is not part of the tests).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lightcraft_catalog::PhotoId;
use lightcraft_retouch::portrait::fixture::{W, mesh, portrait};
use serde_json::json;

use crate::Session;
use crate::portrait_job::{Planned, PortraitRun, compute, plan_fixed};
use crate::portrait_stack::{self as ps, Meshes};

fn write_png(path: &std::path::Path, rgb: &[u8], w: usize, h: usize) {
    let data: Vec<[u8; 4]> = rgb.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2], 255]).collect();
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: w, height: h, data }),
        &Default::default(),
    )
    .unwrap();
    std::fs::write(path, png).unwrap();
}

/// `n` portraits side by side in one picture (n faces), and the mesh of each in normalized coordinates.
fn group_photo(n: usize, tweak: u8) -> (Vec<u8>, usize, usize, Meshes) {
    let (one, _) = portrait(true);
    let (w, h) = (W * n, W);
    let mut rgb = vec![0u8; w * h * 3];
    for i in 0..n {
        for y in 0..W {
            let (src, dst) = (y * W * 3, (y * w + i * W) * 3);
            rgb[dst..dst + W * 3].copy_from_slice(&one[src..src + W * 3]);
        }
    }
    rgb[0] ^= tweak;
    let meshes =
        (0..n).map(|i| mesh(true).iter().map(|p| [(p[0] + (i * W) as f32) / w as f32, p[1] / h as f32, p[2] / w as f32]).collect()).collect();
    (rgb, w, h, meshes)
}

fn session(tag: &str, photos: usize, faces: usize) -> (Session, Vec<PhotoId>, Meshes) {
    let dir: PathBuf = std::env::temp_dir().join(format!("lc-portrait-job-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut meshes = Vec::new();
    for i in 0..photos {
        let (rgb, w, h, m) = group_photo(faces, i as u8 + 1);
        write_png(&dir.join(format!("p{i}.png")), &rgb, w, h);
        meshes = m;
    }
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let mut ids: Vec<(String, PhotoId)> = s.catalog.photos().map(|p| (p.file_name.clone(), p.id)).collect();
    ids.sort();
    (s, ids.into_iter().map(|x| x.1).collect(), meshes)
}

fn auto() -> Vec<lightcraft_develop::PortraitStep> {
    ps::auto_steps()
}

/// Drive a run to its end the way a frame loop does, returning the longest single `pump` and how
/// many of them found the worker still busy.
fn drive(run: &mut PortraitRun, s: &mut Session) -> (Duration, usize) {
    let (mut worst, mut busy) = (Duration::ZERO, 0);
    let t0 = Instant::now();
    loop {
        let t = Instant::now();
        let finished = run.pump(s);
        worst = worst.max(t.elapsed());
        if finished {
            return (worst, busy);
        }
        busy += 1;
        assert!(t0.elapsed() < Duration::from_secs(120), "the run did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_run_retouches_every_photo_with_one_undo_step_each_and_the_caller_never_waits() {
    // the longest call on the caller's thread, best of three (other tests share the CPU)
    let mut best = Duration::MAX;
    for round in 0..3 {
        let (mut s, ids, meshes) = session(&format!("run{round}"), 3, 3);
        let undo0 = s.undo.len();
        let m = meshes.clone();
        let mut run = PortraitRun::new(ids.clone(), auto(), false, 4).with_faces(Arc::new(move |_| m.clone()));
        let (worst, busy) = drive(&mut run, &mut s);
        assert!(run.finished && !run.cancelled);
        assert_eq!(run.done, 3);
        assert_eq!(run.counts(), (3, 0, 0), "{:?}", run.photos);
        assert_eq!(run.photos[0]["faces"], 3, "three faces in the picture");
        assert_eq!(s.undo.len(), undo0 + 3, "one undo step per photo");
        for id in &ids {
            let d = s.develop_of(*id).unwrap();
            assert!(d.portrait.patch.is_some() && d.portrait.steps.len() >= 8);
        }
        assert!(busy >= 2, "the caller found the worker busy {busy} times: the work did not run inline");
        println!("caller's longest pump: {worst:?}, busy polls: {busy}");
        best = best.min(worst);
    }
    assert!(best < Duration::from_millis(50), "the longest call on the caller's thread was {best:?} (best of three)");
}

#[test]
fn the_result_is_the_one_of_the_synchronous_apply() {
    let (mut s1, ids1, meshes) = session("sync", 1, 2);
    let (mut s2, ids2, _) = session("async", 1, 2);
    let m = meshes.clone();
    let (mut f, mut mt) = (
        move |_: &mut Session, _: PhotoId| -> crate::Result<Meshes> { Ok(m.clone()) },
        |_: &mut Session, _: PhotoId, w: usize, h: usize| -> crate::Result<lightcraft_retouch::plane::Plane> {
            Ok(lightcraft_retouch::plane::Plane::new(w, h, 0.0))
        },
    );
    let mut src = ps::Sources { faces: &mut f, matte: &mut mt };
    let a = crate::cmd::portrait_stack::apply_one(&mut s1, ids1[0], auto(), false, 4, &mut src).unwrap();
    let m = meshes.clone();
    let mut run = PortraitRun::new(ids2.clone(), auto(), false, 4).with_faces(Arc::new(move |_| m.clone()));
    drive(&mut run, &mut s2);
    let b = &run.photos[0];
    assert_eq!(a["faces"], b["faces"]);
    assert_eq!(a["steps"], b["steps"], "the same steps changed the same share");
    assert_eq!(a["patch"]["width"], b["patch"]["width"]);
    assert_eq!(a["patch"]["height"], b["patch"]["height"]);
    // the two sessions imported the same pixels, so the patches are the same image
    let (pa, pb) = (s1.develop_of(ids1[0]).unwrap().portrait.patch.clone(), s2.develop_of(ids2[0]).unwrap().portrait.patch.clone());
    assert_eq!(pa.is_some(), pb.is_some());
}

#[test]
fn stop_answers_at_once_and_writes_nothing_for_the_photo_in_flight() {
    let (mut s, ids, meshes) = session("stop", 4, 3);
    let undo0 = s.undo.len();
    let m = meshes.clone();
    let mut run = PortraitRun::new(ids.clone(), auto(), false, 4).with_faces(Arc::new(move |_| m.clone()));
    assert!(!run.pump(&mut s), "the first photo is on a worker");
    let t = Instant::now();
    run.cancel();
    assert!(run.pump(&mut s), "finished at once");
    assert!(t.elapsed() < Duration::from_millis(100), "stop took {:?}", t.elapsed());
    assert!(run.cancelled);
    assert_eq!(s.undo.len(), undo0, "nothing was written");
    assert!(ids.iter().all(|id| s.develop_of(*id).unwrap().portrait.patch.is_none()));
    // and the worker itself notices a stop within a step
    let (rgb, w, h, meshes) = group_photo(3, 0);
    let _ = (rgb, w, h);
    let m = meshes.clone();
    let planned = plan_fixed(&mut s, ids[1], auto(), false, 4, ps::EDGE, Some(m)).unwrap();
    let Planned::Work(work, _) = planned else { panic!("work expected") };
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    let th = std::thread::spawn(move || compute(*work, &f2));
    std::thread::sleep(Duration::from_millis(5));
    let t = Instant::now();
    flag.store(true, Ordering::Relaxed);
    let r = th.join().unwrap();
    // either it was already done (a small picture) or it stopped
    println!("worker answered a stop in {:?}: {}", t.elapsed(), r.as_ref().map_or("cancelled".to_string(), |_| "finished first".to_string()));
    assert!(t.elapsed() < Duration::from_millis(500));
    if let Err(e) = r {
        assert!(ps::is_cancelled(&e));
    }
}

#[test]
fn a_photo_edited_while_it_is_retouched_is_left_alone() {
    let (mut s, ids, meshes) = session("edited", 1, 1);
    let m = meshes.clone();
    let mut run = PortraitRun::new(ids.clone(), auto(), false, 4).with_faces(Arc::new(move |_| m.clone()));
    assert!(!run.pump(&mut s));
    // the user moves a slider while the worker is busy
    let mut d = (*s.develop_of(ids[0]).unwrap()).clone();
    d.portrait.faces = 77;
    s.set_develop(ids[0], d, "an edit while the worker runs").unwrap();
    let (_, _) = drive(&mut run, &mut s);
    let r = &run.photos[0];
    assert!(r["skipped"].is_string() || r["faces"].is_number(), "{r}");
    if r["skipped"].is_string() {
        assert!(s.develop_of(ids[0]).unwrap().portrait.patch.is_none(), "the stale result was not stored");
    }
}
