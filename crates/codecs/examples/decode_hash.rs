//! Decode files and print a hash of the decoded pixels (regression / timing aid).
//! `DUMP_DIR=dir` also writes the sRGB 8-bit RGBA result of each file.
//! Usage: `decode_hash <file>...`  or  `decode_hash @list.txt` (one path per line).
use std::time::Instant;

fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

fn main() {
    let mut paths: Vec<String> = Vec::new();
    for a in std::env::args().skip(1) {
        if let Some(l) = a.strip_prefix('@') {
            paths.extend(std::fs::read_to_string(l).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(String::from));
        } else {
            paths.push(a);
        }
    }
    for p in paths {
        let Ok(bytes) = std::fs::read(&p) else {
            println!("{p}\tREAD-ERROR");
            continue;
        };
        let t = Instant::now();
        match lightcraft_codecs::decode(&bytes, lightcraft_codecs::DecodeOptions::default()) {
            Ok(d) => {
                let ms = t.elapsed().as_secs_f64() * 1e3;
                let mut h = 0xcbf29ce484222325u64;
                fnv(&mut h, bytemuck::cast_slice(&d.image.data));
                let mut ha = 0xcbf29ce484222325u64;
                if let Some(a) = &d.alpha {
                    fnv(&mut ha, bytemuck::cast_slice(&a.data));
                }
                if let Ok(dir) = std::env::var("DUMP_DIR") {
                    let o = d.to_srgb8();
                    let name = std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let _ = std::fs::write(format!("{dir}/{name}.{}x{}.rgba", o.width, o.height), bytemuck::cast_slice::<_, u8>(&o.data));
                }
                println!(
                    "{p}\t{}x{}\tbits={}\torient={}\talpha={}\tpix={h:016x}\talphahash={ha:016x}\t{ms:.0}ms",
                    d.width, d.height, d.bit_depth, d.orientation, d.has_alpha
                );
            }
            Err(e) => println!("{p}\tERR {e}"),
        }
    }
}
