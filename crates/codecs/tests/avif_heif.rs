//! AVIF and HEIC still-image decoding on small committed fixtures (`tests/data`, all < 12 KB).
//!
//! Provenance (all external tools are test-time only, nothing here links them):
//! - `s_*` / `g_*` / `a_*` / `f_mono`: encoded with libheif's `heif-enc` (aom for AVIF, x265 for HEIC) or
//!   ffmpeg (libaom, gray) from synthetic sources (`s_src.png`, `g_src.png`, ffmpeg `testsrc2`).
//! - `*.ref.png`: the same files decoded by libheif (`heif-convert`); 4:4:4 / monochrome decode is exact
//!   (libheif upsamples 4:2:0 chroma by replication, we interpolate, so 4:2:0 is checked by PSNR against the
//!   source instead).
//! - `p_*`: patched copies that add `irot` / `imir` / `clap` properties or an auxiliary alpha item to the
//!   primary item (their pixels are those of the file they were made from).

#![cfg(any(feature = "avif-decode", feature = "heif"))]

use lightcraft_codecs::*;

macro_rules! fx {
    ($n:literal) => {
        include_bytes!(concat!("data/", $n)) as &[u8]
    };
}

fn dec(b: &[u8]) -> Decoded {
    decode(b, DecodeOptions::default()).expect("fixture decodes")
}

fn psnr(a: &Decoded, src: &Decoded) -> f64 {
    let (a, s) = (a.to_srgb8(), src.to_srgb8());
    assert_eq!((a.width, a.height), (s.width, s.height));
    let mut se = 0.0f64;
    for (p, q) in a.data.iter().zip(&s.data) {
        for c in 0..3 {
            let d = p[c] as f64 - q[c] as f64;
            se += d * d;
        }
    }
    let mse = se / (a.data.len() * 3) as f64;
    if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0 * 255.0 / mse).log10() }
}

/// Max per-channel difference of the 8-bit sRGB renderings (alpha included).
fn max_diff(a: &Decoded, b: &Decoded) -> u8 {
    let (a, b) = (a.to_srgb8(), b.to_srgb8());
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data.iter().zip(&b.data).flat_map(|(p, q)| (0..4).map(move |c| p[c].abs_diff(q[c]))).max().unwrap_or(0)
}

#[cfg(feature = "avif-decode")]
mod avif {
    use super::*;

    #[test]
    fn capability_flag() {
        assert!(Format::Avif.can_decode());
        assert_eq!(sniff(fx!("a_444.avif")), Some(Format::Avif));
    }

    #[test]
    fn exact_against_libheif_444_mono_grid_alpha_clap() {
        for (f, r) in [
            (fx!("a_444.avif"), fx!("a_444.ref.png")),
            (fx!("a_alpha444.avif"), fx!("a_alpha444.ref.png")),
            (fx!("f_mono.avif"), fx!("f_mono.ref.png")),
            (fx!("g_grid444_alpha.avif"), fx!("g_grid444_alpha.ref.png")),
            (fx!("p_clap_int.avif"), fx!("p_clap_int.ref.png")),
        ] {
            let (a, b) = (dec(f), dec(r));
            assert!(max_diff(&a, &b) <= 1, "max diff {}", max_diff(&a, &b));
        }
    }

    #[test]
    fn alpha_planes() {
        let d = dec(fx!("a_alpha444.avif"));
        assert!(d.has_alpha);
        let a = d.alpha.as_ref().expect("alpha");
        assert!(a.data[0] < 0.05 && a.data[a.width - 1] > 0.95, "horizontal ramp");
        // The grid's alpha is carried by one alpha item per tile and ramps vertically.
        let g = dec(fx!("g_grid444_alpha.avif"));
        let a = g.alpha.as_ref().expect("grid alpha");
        assert_eq!((g.width, g.height), (100, 70));
        assert!(a.data[0] < 0.05 && a.data[a.width * (a.height - 1)] > 0.95, "vertical ramp");
        // Premultiplied flag (`prem` reference) is honoured without error.
        assert!(dec(fx!("p_alpha_prem.avif")).has_alpha);
    }

    #[test]
    fn subsampling_and_bit_depth() {
        let src = dec(fx!("s_src.png"));
        let d = dec(fx!("s_420.avif"));
        assert_eq!(d.bit_depth, 8);
        assert!(psnr(&d, &src) > 40.0, "4:2:0 psnr {}", psnr(&d, &src));
        assert!(psnr(&dec(fx!("s_422.avif")), &src) > 38.0);
        let d10 = dec(fx!("s_420_10.avif"));
        assert_eq!(d10.bit_depth, 10);
        assert!(psnr(&d10, &src) > 40.0, "10-bit psnr {}", psnr(&d10, &src));
        assert!(dec(fx!("f_mono.avif")).grayscale);
    }

    #[test]
    fn grid_420_with_tile_alpha() {
        let g = dec(fx!("g_grid420_alpha.avif"));
        assert_eq!((g.width, g.height), (100, 70));
        assert!(g.has_alpha);
        // Same source and tiling as the 4:4:4 grid: identical alpha planes, colour within chroma-subsampling loss
        // (testsrc2 has hard coloured edges, hence the low bar; a misplaced tile would land near 10 dB).
        let g444 = dec(fx!("g_grid444_alpha.avif"));
        assert_eq!(g.alpha.as_ref().expect("alpha").data, g444.alpha.as_ref().expect("alpha").data);
        let p = psnr(&g, &g444);
        assert!(p > 20.0, "4:2:0 vs 4:4:4 grid psnr {p}");
    }

    #[test]
    fn orientation_from_irot_imir() {
        let (a, b) = (dec(fx!("p_irot90.avif")), dec(fx!("p_irot3_mir.avif")));
        assert_eq!((a.orientation, b.orientation), (8, 5));
        // Not applied to the pixels.
        assert_eq!((a.width, a.height), (203, 131));
    }

    #[test]
    fn clap_geometry_and_errors() {
        let c = dec(fx!("p_clap_int.avif"));
        assert_eq!((c.width, c.height, c.source_width, c.source_height), (63, 47, 63, 47));
        assert!(matches!(decode(fx!("p_clap_bad.avif"), DecodeOptions::default()), Err(Error::Malformed(Format::Avif, _))));
        assert!(matches!(decode(fx!("p_clap_after_irot.avif"), DecodeOptions::default()), Err(Error::Unsupported(Format::Avif, _))));
    }

    #[test]
    fn max_size_and_pixel_limit() {
        let d = decode(fx!("a_444.avif"), DecodeOptions::fit(50, 50)).expect("fit");
        assert!(d.width <= 50 && d.height <= 50);
        assert!(matches!(decode(fx!("a_444.avif"), DecodeOptions { max_size: None, max_pixels: 100 }), Err(Error::TooLarge(..))));
    }

    /// Encode with our own encoder (ravif) and decode again.
    #[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
    mod roundtrip {
        use super::*;
        use lightcraft_raster::Rgba8;

        fn smooth(w: usize, h: usize) -> Rgba8 {
            Rgba8::from_fn(w, h, |x, y| {
                let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                [(fx * 255.0) as u8, (fy * 255.0) as u8, ((1.0 - fx) * 200.0) as u8, (60.0 + 190.0 * fx * fy) as u8]
            })
        }

        fn encode_png_ref(img: &Rgba8) -> Decoded {
            let png = encode_png(&EncodeImage::rgba8(img), &EncodeMeta::default()).expect("png");
            dec(&png)
        }

        #[test]
        fn ravif_8bit_roundtrip_with_alpha() {
            let img = smooth(97, 61);
            let avif = encode_avif(&EncodeImage::rgba8(&img), 90, 10, &EncodeMeta::default()).expect("encode");
            let d = dec(&avif);
            // The encoder widens 8-bit input to a 10-bit AV1 stream.
            assert_eq!((d.width, d.height, d.bit_depth), (97, 61, 10));
            assert!(d.has_alpha);
            let p = psnr(&d, &encode_png_ref(&img));
            assert!(p > 35.0, "8-bit round-trip psnr {p}");
            // Alpha survives within lossy tolerance.
            let (a, r) = (d.alpha.as_ref().expect("alpha"), encode_png_ref(&img).alpha.expect("ref alpha"));
            let worst = a.data.iter().zip(&r.data).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
            assert!(worst < 0.06, "alpha error {worst}");
        }

        #[test]
        fn ravif_10bit_roundtrip() {
            let (w, h) = (64usize, 48usize);
            let px: Vec<u16> = (0..w * h).flat_map(|i| [((i % w) * 65535 / w) as u16, ((i / w) * 65535 / h) as u16, 30000]).collect();
            let avif = encode_avif(&EncodeImage::new(w as u32, h as u32, 3, Samples::U16(&px)), 90, 10, &EncodeMeta::default()).expect("encode");
            let d = dec(&avif);
            assert_eq!((d.width, d.height, d.bit_depth), (64, 48, 10));
            // Compare in the linear working buffer against the 16-bit source (display-encoded values / 65535).
            let mut worst = 0.0f32;
            for (i, p) in d.image.data.iter().enumerate() {
                let src = [px[i * 3], px[i * 3 + 1], px[i * 3 + 2]].map(|v| lightcraft_color::transfer::srgb_to_linear(v as f32 / 65535.0));
                for c in 0..3 {
                    worst = worst.max((p[c] - src[c]).abs());
                }
            }
            assert!(worst < 0.03, "10-bit linear error {worst}");
        }
    }
}

#[cfg(feature = "heif")]
mod heic {
    use super::*;

    #[test]
    fn eight_and_ten_bit() {
        let src = dec(fx!("s_src.png"));
        let d8 = dec(fx!("s_8.heic"));
        let d10 = dec(fx!("s_10.heic"));
        assert_eq!((d8.bit_depth, d10.bit_depth), (8, 10));
        assert!(psnr(&d8, &src) > 40.0, "8-bit psnr {}", psnr(&d8, &src));
        assert!(psnr(&d10, &src) > 40.0, "10-bit psnr {}", psnr(&d10, &src));
    }

    #[test]
    fn clap_is_applied() {
        // libheif wrote a 1-pixel `clap` (204x132 -> 203x131) and the patch added a centred 100x60 one.
        let d = dec(fx!("p_h_clap.heic"));
        assert_eq!((d.width, d.height), (100, 60));
    }

    #[test]
    fn auxiliary_alpha() {
        let d = dec(fx!("p_h_alpha420.heic"));
        assert!(d.has_alpha);
        assert_eq!((d.width, d.height), (203, 131));
        // Apple/libheif code the alpha plane as 4:0:0 HEVC, which filmcraft-hevc does not decode:
        // the image still decodes, without alpha.
        let m = dec(fx!("h_alpha.heic"));
        assert!(!m.has_alpha);
    }
}

/// Garbage in, a `Result` out, never a panic or a hang. Goes through the public (panic-guarded) `decode`:
/// the sibling `filmcraft-av1` still panics on some corrupt streams, which the guard turns into an error.
mod robustness {
    use super::*;
    use proptest::prelude::*;
    use std::sync::OnceLock;

    fn seeds() -> &'static Vec<Vec<u8>> {
        static S: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
        S.get_or_init(|| {
            [
                fx!("s_420.avif"),
                fx!("s_420_10.avif"),
                fx!("a_444.avif"),
                fx!("f_mono.avif"),
                fx!("g_grid420_alpha.avif"),
                fx!("g_grid444_alpha.avif"),
                fx!("a_alpha444.avif"),
                fx!("p_clap_int.avif"),
                fx!("s_8.heic"),
                fx!("s_10.heic"),
                fx!("p_h_alpha420.heic"),
                fx!("p_h_clap.heic"),
            ]
            .iter()
            .map(|b| b.to_vec())
            .collect()
        })
    }

    fn try_decode(b: &[u8]) {
        let _ = decode(b, DecodeOptions { max_size: None, max_pixels: 1 << 22 });
    }

    #[test]
    fn every_truncation_of_every_seed_is_an_error_or_ok() {
        for s in seeds() {
            // Every cut in the container, then every 7th inside the coded data.
            for n in (0..s.len()).filter(|n| *n < 400 || n % 7 == 0) {
                try_decode(&s[..n]);
            }
        }
    }

    #[test]
    fn truncated_files_are_errors() {
        for s in seeds() {
            let f = sniff(s).expect("sniff");
            assert!(decode(&s[..s.len() / 2], DecodeOptions::default()).is_err(), "{f:?}");
        }
    }

    /// One flipped byte in `g_grid420_alpha.avif` makes `filmcraft-av1` panic (`plane_residual_size`
    /// returns BLOCK_INVALID, `tile.rs` `get_tx_size`). With frame worker threads that left the caller
    /// waiting forever; decoding on the calling thread turns it into an error.
    #[test]
    fn av1_panic_on_corrupt_tile_is_an_error_not_a_hang() {
        let r = decode(fx!("av1_frame_worker_panic.avif"), DecodeOptions::default());
        assert!(r.is_err(), "{:?}", r.map(|d| (d.width, d.height)));
    }

    #[test]
    fn random_bytes_behind_ftyp() {
        for brand in [&b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf"[..], &b"\0\0\0\x18ftypheic\0\0\0\0mif1heic"[..]] {
            let mut x = 0x1234_5678u32;
            for _ in 0..300 {
                let mut b = brand.to_vec();
                for _ in 0..200 {
                    x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    b.push((x >> 24) as u8);
                }
                try_decode(&b);
            }
        }
    }

    proptest! {
        // FUZZ_CASES=20000 for a longer run.
        #![proptest_config(ProptestConfig { cases: std::env::var("FUZZ_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(600), ..ProptestConfig::default() })]

        #[test]
        fn mutated_valid_files(which in 0usize..12, flips in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..16)) {
            let mut b = seeds()[which].clone();
            for (i, v) in flips {
                let i = i.index(b.len());
                b[i] = v;
            }
            try_decode(&b);
        }
    }
}
