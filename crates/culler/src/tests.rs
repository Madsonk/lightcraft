use super::synthetic::{Plant, album};
use super::*;

fn run(genre: Genre) -> (synthetic::Album, Vec<Verdict>) {
    let a = album(4, 30, 7);
    let v = judge(&a.shots, &Options::for_genre(genre));
    (a, v)
}

#[test]
fn planted_faults_are_rejected_and_good_shots_are_not() {
    let (a, v) = run(Genre::Wedding);
    let (mut hit, mut total, mut false_reject, mut good) = (0, 0, 0, 0);
    for (p, v) in a.plants.iter().zip(&v) {
        if p.must_reject() {
            total += 1;
            hit += (v.bucket == Bucket::Rejected) as usize;
        }
        if *p == Plant::Good {
            good += 1;
            false_reject += (v.bucket == Bucket::Rejected) as usize;
        }
    }
    assert!(total > 20 && good > 20, "album too small: {total} {good}");
    assert_eq!(hit, total, "every planted fault is rejected");
    assert_eq!(false_reject, 0, "no good shot is rejected");
    // a blurred face is evidence, not proof: it sends the photo to review
    for (p, v) in a.plants.iter().zip(&v) {
        if *p == Plant::FaceBlurred {
            assert_eq!(v.bucket, Bucket::Review, "{v:?}");
        }
    }
}

#[test]
fn burst_keeps_exactly_the_best_frame() {
    let (a, v) = run(Genre::General);
    for (i, p) in a.plants.iter().enumerate() {
        match p {
            Plant::BurstBest => {
                let x = &v[i];
                assert_eq!(x.group_rank, 0, "{x:?}");
                assert_eq!(x.group_size, 3);
                assert!(x.reasons.contains(&Reason::BestOfGroup));
                assert!(matches!(x.bucket, Bucket::Selected | Bucket::Highlight), "{x:?}");
            }
            Plant::BurstWorse => {
                assert!(v[i].reasons.contains(&Reason::Duplicate));
                assert_eq!(v[i].bucket, Bucket::Rejected);
            }
            _ => {}
        }
    }
}

#[test]
fn closed_eyes_reason_and_flags() {
    let (a, v) = run(Genre::Portrait);
    let i = a.plants.iter().position(|p| *p == Plant::ClosedEyes).unwrap();
    assert!(v[i].reasons.contains(&Reason::ClosedEyes));
    assert!(v[i].flags().contains(&"closed_eyes"));
    assert_eq!(v[i].stars, 1);
    let j = a.plants.iter().position(|p| *p == Plant::FaceBlurred).unwrap();
    assert!(v[j].reasons.contains(&Reason::FaceBlurred), "{:?}", v[j]);
    let k = a.plants.iter().position(|p| *p == Plant::Dark).unwrap();
    assert!(v[k].reasons.contains(&Reason::Underexposed));
}

#[test]
fn sport_ignores_eyes_and_keeps_sequences_for_review() {
    let (a, v) = run(Genre::Sport);
    let i = a.plants.iter().position(|p| *p == Plant::ClosedEyes).unwrap();
    assert!(!v[i].reasons.contains(&Reason::ClosedEyes) && !v[i].reasons.contains(&Reason::HalfClosedEyes));
    let w = a.plants.iter().position(|p| *p == Plant::BurstWorse).unwrap();
    assert_eq!(v[w].bucket, Bucket::Review, "sport keeps the rest of a sequence for review");
}

#[test]
fn highlights_are_a_minority_of_the_selected() {
    let (_, v) = run(Genre::General);
    let hi = v.iter().filter(|x| x.bucket == Bucket::Highlight).count();
    let sel = v.iter().filter(|x| matches!(x.bucket, Bucket::Highlight | Bucket::Selected)).count();
    assert!(hi > 0 && hi * 3 <= sel, "{hi} highlights of {sel}");
    assert!(v.iter().filter(|x| x.bucket == Bucket::Highlight).all(|x| x.stars == 5 && x.score >= 55.0));
}

#[test]
fn scenes_follow_the_hours_and_are_chronological() {
    let a = album(4, 30, 3);
    let scenes = scene::segment(&a.shots, &Genre::General.profile());
    // each planted scene is one judged scene, in order
    for (i, s) in scenes.iter().enumerate() {
        assert_eq!(*s, a.scene_of[i], "shot {i}");
    }
}

#[test]
fn long_scene_is_split_by_appearance() {
    // one continuous scene of two very different looks, 40 shots each
    let mut shots = Vec::new();
    let a = album(1, 1, 1);
    let rng_sig = |seed: usize| -> Vec<f32> {
        let mut v: Vec<f32> = (0..SIGNATURE_LEN).map(|k| (((k * 31 + seed * 17) % 13) as f32) - 6.0).collect();
        let m = v.iter().sum::<f32>() / 64.0;
        v.iter_mut().for_each(|x| *x -= m);
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= n);
        v
    };
    let (look_a, look_b) = (rng_sig(1), rng_sig(5));
    for i in 0..80usize {
        let mut s = a.shots[0].clone();
        s.time = Some(1_000 + i as i64 * 20);
        s.signature = if i < 40 { look_a.clone() } else { look_b.clone() };
        shots.push(s);
    }
    let scenes = scene::segment(&shots, &Genre::General.profile());
    assert_ne!(scenes[0], scenes[79], "two looks, two scenes");
    assert!(scenes[..40].iter().all(|s| *s == scenes[0]) && scenes[40..].iter().all(|s| *s == scenes[79]));
}

#[test]
fn grouping_slider_changes_how_much_is_grouped() {
    let a = album(2, 30, 9);
    let groups = |g: f32| {
        let mut o = Options::for_genre(Genre::General);
        o.grouping = g;
        judge(&a.shots, &o).iter().filter(|v| v.group.is_some()).count()
    };
    assert!(groups(1.0) >= groups(0.5) && groups(0.5) >= groups(0.0));
    assert!(groups(0.5) > 0, "bursts are grouped at the default setting");
    assert!(groups(1.0) > groups(0.0), "the slider moves the result");
}

#[test]
fn key_people_limit_judging_to_their_faces() {
    let mut a = album(1, 6, 4);
    // Alice (first face) has her eyes closed in shot 0, Bob (second) in shot 1
    for (i, who) in [(0usize, "Alice"), (1, "Bob")] {
        let faces = a.shots[i].faces.as_mut().unwrap();
        faces.clear();
        faces.push(Face { rect: [0.2, 0.2, 0.4, 0.5], open: [0.05, 0.05], sharp: [80.0, 80.0], judged: true, person: Some(who.into()) });
    }
    let mut o = Options::for_genre(Genre::Wedding);
    o.key_people = vec!["alice".into()];
    let v = judge(&a.shots, &o);
    assert!(v[0].reasons.contains(&Reason::ClosedEyes));
    assert!(!v[1].reasons.contains(&Reason::ClosedEyes), "Bob is not a key person");
    assert!(v[1].reasons.contains(&Reason::NoKeyFace));
    assert_eq!(v[1].bucket, Bucket::Review);
}

#[test]
fn faces_not_analyzed_is_said_and_not_penalised() {
    let mut a = album(1, 3, 2);
    for s in &mut a.shots {
        s.faces = None;
    }
    let v = judge(&a.shots, &Options::default());
    assert!(v.iter().all(|x| x.reasons.contains(&Reason::FacesNotAnalyzed)));
    assert!(v.iter().any(|x| matches!(x.bucket, Bucket::Selected | Bucket::Highlight)));
}

#[test]
fn hostile_input_never_panics() {
    let nan = Shot {
        sharpness: f32::NAN,
        clipped: f32::INFINITY,
        exposure: [f32::NAN; 3],
        signature: vec![f32::NAN; 64],
        time: Some(i64::MAX),
        ..Default::default()
    };
    let short = Shot {
        signature: vec![1.0; 3],
        time: Some(i64::MIN),
        faces: Some(vec![Face { open: [f32::NAN, 0.0], sharp: [f32::NAN, 0.0], judged: true, ..Default::default() }]),
        ..Default::default()
    };
    let shots = vec![nan.clone(), short, nan, Shot::default()];
    for g in Genre::ALL {
        let mut o = Options::for_genre(g);
        o.grouping = f32::NAN;
        let v = judge(&shots, &o);
        assert_eq!(v.len(), 4);
        assert!(v.iter().all(|x| x.score.is_finite() && (1..=5).contains(&x.stars)));
    }
    assert!(judge(&[], &Options::default()).is_empty());
    assert_eq!(scene::kmeans(&[], 3, 5), Vec::<usize>::new());
}

#[test]
fn verdict_round_trips_through_json() {
    let (_, v) = run(Genre::School);
    let s = serde_json::to_string(&v[3]).unwrap();
    assert_eq!(serde_json::from_str::<Verdict>(&s).unwrap(), v[3]);
    assert_eq!(Genre::parse("Mariage"), Some(Genre::Wedding));
    assert_eq!(Genre::parse("nope"), None);
}

#[test]
fn background_faces_are_shown_but_not_judged() {
    let mut a = album(1, 3, 5);
    let faces = a.shots[0].faces.as_mut().unwrap();
    faces.clear();
    faces.push(Face { rect: [0.2, 0.2, 0.5, 0.6], open: [0.30, 0.30], sharp: [80.0, 80.0], judged: true, person: None });
    // a small, blurred, closed-eyed guest behind
    faces.push(Face { rect: [0.7, 0.3, 0.76, 0.4], open: [0.05, 0.05], sharp: [5.0, 5.0], judged: true, person: None });
    let v = judge(&a.shots, &Options::for_genre(Genre::Wedding));
    assert!(!v[0].reasons.contains(&Reason::ClosedEyes) && !v[0].reasons.contains(&Reason::FaceBlurred), "{:?}", v[0].reasons);
    assert_eq!(v[0].faces.len(), 2);
    assert!(v[0].faces[0].counted && !v[0].faces[1].counted);
    let mut seen = v[0].reasons.clone();
    seen.dedup();
    assert_eq!(seen.len(), v[0].reasons.len(), "no repeated reason");
}

#[test]
fn face_size_is_a_share_of_the_long_edge() {
    let mut a = album(1, 3, 6);
    // 3:1 panorama; a face 0.03 wide but 0.09 tall is 3 % of the long edge: too small to judge
    a.shots[0].aspect = 3.0;
    let faces = a.shots[0].faces.as_mut().unwrap();
    faces.clear();
    faces.push(Face { rect: [0.2, 0.2, 0.23, 0.29], open: [0.05, 0.05], sharp: [80.0, 80.0], judged: true, person: None });
    let v = judge(&a.shots, &Options::for_genre(Genre::Wedding));
    assert!(!v[0].faces[0].counted, "{:?}", v[0].faces);
    assert!(!v[0].reasons.contains(&Reason::ClosedEyes));
}
