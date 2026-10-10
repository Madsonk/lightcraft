//! Developer tool: run the face model on an image and draw named groups of mesh points, to check
//! the indices `facegeo` relies on. `cargo run --release -p lightcraft-retouch --example mesh_dump -- <model.task> <image> <out.png>`
use lightcraft_facemesh::mediapipe::{FaceLandmarker, Frame};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let task = std::fs::read(&a[1]).expect("model");
    let m = FaceLandmarker::from_task(&task).expect("load");
    let img = image::open(&a[2]).expect("image").to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let rgb: Vec<[f32; 3]> = img.pixels().map(|p| [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]).collect();
    let meshes = m.find_all(Frame { rgb: &rgb, w, h }).expect("find");
    println!("{} faces", meshes.len());
    let mut out = img.clone();
    let groups: Vec<(&str, [u8; 3], Vec<usize>)> = vec![
        ("nose ridge", [255, 0, 0], vec![168, 6, 197, 195, 5, 4, 1]),
        ("cheek cand", [0, 255, 0], vec![205, 425, 50, 280, 117, 346, 123, 352, 116, 345, 187, 411]),
        ("forehead", [0, 0, 255], vec![10, 151, 9, 8, 107, 336, 67, 297, 109, 338]),
        ("chin", [255, 255, 0], vec![152, 175, 199, 200]),
        ("iris", [0, 255, 255], vec![468, 469, 470, 471, 472, 473, 474, 475, 476, 477]),
        ("lower lids", [255, 0, 255], vec![362, 382, 381, 380, 374, 373, 390, 249, 263, 33, 7, 163, 144, 145, 153, 154, 155, 133]),
        ("temples", [255, 255, 255], vec![21, 251, 127, 356, 234, 454]),
        ("nose wings", [255, 128, 0], vec![129, 358, 98, 327, 64, 294, 48, 278]),
    ];
    for mesh in &meshes {
        let sz = ((mesh.bbox()[2] - mesh.bbox()[0]) / 120.0).max(1.0) as i32;
        for (_, col, idx) in &groups {
            for &i in idx {
                let p = mesh.points[i];
                for dy in -sz..=sz {
                    for dx in -sz..=sz {
                        let (x, y) = (p[0] as i32 + dx, p[1] as i32 + dy);
                        if x >= 0 && y >= 0 && (x as u32) < out.width() && (y as u32) < out.height() {
                            out.put_pixel(x as u32, y as u32, image::Rgb(*col));
                        }
                    }
                }
            }
        }
        let zs: Vec<f32> = mesh.points.iter().map(|p| p[2]).collect();
        println!(
            "bbox {:?} z range {:.1}..{:.1} nose tip z {:.1}",
            mesh.bbox(),
            zs.iter().cloned().fold(f32::MAX, f32::min),
            zs.iter().cloned().fold(f32::MIN, f32::max),
            mesh.points[1][2]
        );
    }
    out.save(&a[3]).expect("save");
}
