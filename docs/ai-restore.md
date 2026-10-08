# AI denoise and 2x super-resolution

LightCraft denoises with **SCUNet** and enlarges 2x with **SPAN**, both run in pure Rust by
`crates/restore` on [candle](https://github.com/huggingface/candle): on the GPU through Metal on
macOS, on the CPU elsewhere. No C or C++ is compiled (`cargo tree -p lightcraft-restore` shows
candle, `gemm`, `rayon`, `half` and the project's own `lightcraft-fetch`, all Rust).

**The models are optional and are not part of LightCraft.** Nothing requires them: without them,
every other feature works, and an export that asks for AI denoise or super-resolution says what is
missing and stops (it never silently skips the step). They are downloaded only when the user asks.

## Using them

- **AI Denoise** is the develop control `enhance.denoise` (0 – 100 %, the *strength*: the result
  is `original + strength × (denoised − original)`), under Noise Reduction in the Detail panel.
- **Super-resolution** is the develop setting `enhance.super_resolution` (a boolean, set with
  `develop.merge {"settings": {"enhance": {"super_resolution": true}}}`): the exported image is
  twice as wide and as tall.
- Both are applied **to the finished render, in exports only** (the app's Export, `app.export`,
  and the CLI), never in a preview, because they take minutes on a full-size photo: denoise first,
  at the size of the render (where the noise is), then the enlargement. They work on 8-bit and on
  16-bit / float renders, in whatever output colour space.

```sh
lightcraft-cli render IMG_0049.CR2 -o out.jpg --denoise 100          # AI denoise at 100 %
lightcraft-cli render photo.jpg -o big.png --size 1920 --upscale 2   # render 1920 px, enlarge to 3840 px
```

The built-in noise reduction (`detail.nr*`) still runs first, in the develop pipeline; the AI step
sees its result. Sharpening also runs before it: with strong AI denoise, lower the sharpening.

## Getting the models

```sh
lightcraft-cli run restore.model.status
lightcraft-cli run restore.model.download model=denoise acknowledged=true
lightcraft-cli run restore.model.download model=upscale acknowledged=true
```

`acknowledged: true` is refused unless passed: give it only after the user agreed to the licence
below. The download is the one `lightcraft-fetch` does for SAM 3 (pure-Rust TLS, resume, timeouts,
`<name>.part` renamed only when complete): each file must have the exact size and SHA-256 below, a
file that doesn't match is deleted. It goes to the models folder: `LIGHTCRAFT_RESTORE_DIR`, else
`models/restore` in the settings folder (`~/Library/Application Support/LightCraft/` on macOS,
`%APPDATA%\LightCraft\`, `~/.config/lightcraft/`). To install by hand, put the files there.
`LIGHTCRAFT_RESTORE_MIRRORS` (base URLs) or `restore-mirrors.txt` in that folder add download
locations tried before the authors' own.

| Model | File | Size | SHA-256 | Source |
|---|---|---:|---|---|
| `denoise` (SCUNet "real", PSNR) | `scunet_color_real_psnr.pth` | 71 982 841 | `fa78899b…09c2e` | `https://github.com/cszn/KAIR/releases/download/v1.0/` |
| `upscale` (SPAN 2x NomosUni) | `2xNomosUni_span_multijpg.safetensors` | 4 461 056 | `bee2a9c0…c92c` | `https://huggingface.co/Phips/2xNomosUni_span_multijpg/resolve/main/` |

(full digests in `crates/restore/src/files.rs`).

Environment (all optional): `LIGHTCRAFT_RESTORE_DIR` (models folder), `LIGHTCRAFT_RESTORE_MIRRORS`
(extra download base URLs), `LIGHTCRAFT_RESTORE_DEVICE=cpu` (skip Metal), `LIGHTCRAFT_RESTORE_TILE` and
`LIGHTCRAFT_RESTORE_OVERLAP` (tile size and overlap in px; tile sizes are rounded up to a multiple of 64
for SCUNet).

In the desktop app the **AI Denoise** slider sits under Noise Reduction in the Detail panel (shown
when the build has AI restore), with a note when the model is missing. There is **no download
dialog yet**: until one exists, the model comes from `restore.model.download` (control channel,
`lightcraft-cli run --connect`) or by hand. The 2x super-resolution has no UI toggle.

## Licences (read from the sources, 8 Oct 2026)

LightCraft's code is MIT OR Apache-2.0. `crates/restore` ports Apache-2.0 code and is therefore
**Apache-2.0** only (like `crates/segment`; see `NOTICE`). **The weights are not LightCraft's** and
are never in this repository, its releases or installers.

- **SCUNet**: code Apache-2.0 (`cszn/SCUNet`, GitHub licence API and `LICENSE`). The weights
  `scunet_color_real_psnr.pth` are an asset of the release `v1.0` of the authors' `cszn/KAIR`
  repository (MIT) and carry **no licence statement of their own**, neither on the SCUNet page nor
  in the release. Training data (README and paper): high-quality photographs from public datasets
  with synthetic degradation (no paired SIDD / DND data). So: the code licence is clear, the
  weights are "published by the authors for use" without an explicit grant. That is the weakest
  point of this choice; the in-app text says it. Anyone redistributing the file should ask the
  authors (Kai Zhang et al.) for an explicit licence.
- **SPAN**: code Apache-2.0 (`hongyuanyu/SPAN`). The weights are by Philip Hofmann, **CC BY 4.0**,
  declared on the model page (`license: cc-by-4.0`): attribution is required (this file and
  `NOTICE` give it). Trained on his NomosUni dataset (2 989 images), from
  `2x_span_anime_pretrain.pth`; the dataset's own image rights are not stated by the author.
- **Not chosen**: NAFNet's code is MIT (`LICENSE`), but its SIDD weights are only on Google Drive /
  Baidu with no licence statement, and SIDD is smartphone noise in sRGB, not DSLR RAW. A
  `Heliosoph/scunet-onnx` mirror on Hugging Face declares Apache-2.0 for SCUNet, but it is a third
  party's re-export of the same weights; the authors' own file is the cleaner source.

## How it works

- `scunet.rs`, `span.rs`: ports of `network_scunet.py` (KAIR/SCUNet) and `span_arch.py` (SPAN).
  SCUNet is a U-Net of "swin-conv" blocks (8 × 8 window attention with relative position bias,
  alternating regular and shifted windows with the same masks as the reference, next to residual
  3 × 3 convolutions), blind: no noise level to give. SPAN folds each of its re-parameterised
  `Conv3XC` into one 3 × 3 convolution when loading, as the reference does at inference.
- `tiles.rs`: any image size, bounded memory. Tiles of 512 px (SCUNet padded to a multiple of 64)
  with 48 px of overlap (24 for SPAN), border-replicated, run one at a time, blended with a linear
  ramp over the overlap and normalised by the summed weights (no seams, and the identity network
  returns the input exactly: unit tests on odd sizes). Peak memory is the input, the output and a
  weight plane plus one tile's activations.
- Domain: both networks were trained on gamma-encoded images. The render is converted as it is
  (8-bit value / 255; 16-bit value / 65535; float renders through the output space's transfer
  curve), the result is converted back, alpha is kept. Strength is the blend above.
- Checked against PyTorch: the SPAN output equals the PyTorch reference's on a 960 × 720 photo to
  73.6 dB PSNR (8-bit rounding), through the tiling code. SCUNet gives the expected gain on a
  noisy test (below); it was not compared bit-for-bit with PyTorch.

## Measurements (Apple M4, 10 cores, Metal; everything here is from `restore_bench` / `lightcraft-cli`)

Synthetic: a clean 1536 × 1152 photo plus Poisson-Gaussian noise in linear light (variance
0.01 x + 0.0001), SCUNet on Metal:

| | PSNR | SSIM (luma) |
|---|---:|---:|
| noisy | 24.78 dB | 0.631 |
| SCUNet denoised | 34.17 dB | 0.915 |

Real photos: Canon EOS 5D Mark III, ISO 1250, `IMG_0049`, `IMG_0111`, `IMG_0118` (copies of the
CR2s, read only), 1278 × 960 crops at 100 % rendered with `--denoise 100`, looked at side by side
(`/Volumes/Madson/ArtCraft/_tests/restore/cmp_*.png`):

- Grain and colour speckle are gone in flat areas and in the dark background of `IMG_0118`
  (high-frequency residual of a flat patch, std in 8-bit levels: 1.50 → 0.17, 0.75 → 0.28,
  0.95 → 0.15 for 0118, 0049, 0111). The mean colour moves by less than 0.2 level, except in the
  very dark `IMG_0118` (−0.6 … −1 level).
- Detail is kept on the edges that carry it (plate rim, parsley, salt crystals, knitted cloth),
  but **fine low-contrast texture is smoothed**: the surface of the meat and the purée in
  `IMG_0111` look slightly "painted", and flat gradients become perfectly smooth. At 100 % this
  is a visible, deliberate trade; use a strength of 50-70 % to keep some grain.

SPAN 2x on a 960 × 720 bicubic downscale of a 1920 × 1440 photo, against the original:

| | PSNR | SSIM (luma) |
|---|---:|---:|
| Lanczos (ffmpeg) | 34.72 dB | 0.945 |
| bicubic | 34.32 dB | 0.942 |
| SPAN 2x | 31.73 dB | 0.928 |

SPAN scores **lower** than Lanczos on these metrics and looks crisper (fur, beard, lettering in
`cmp_sr.png`): it invents plausible detail, which the metrics punish. It is a look, not a
fidelity, upgrade. Do not use it where the exact pixels matter.

Speed and memory (`restore_bench`, `lightcraft-cli`; the Metal device in candle 0.9.2):

| | |
|---|---|
| SCUNet, one 512 × 512 tile | ≈ 7 – 8 s on Metal, ≈ 14.6 s on the 10 CPU cores |
| SCUNet, 1.5 Mpx (1536 × 1152) | 105 s on Metal |
| 1278 × 960 crop of a CR2, `--denoise 100` | 55 – 77 s, 0.43 – 0.74 GB resident |
| 22 Mpx photo (5760 × 3840, `IMG_0049.CR2`), `--denoise 100`, Metal | **1118 s (18.6 min)**, 1.30 GB resident, 3.4 GB peak footprint (an earlier run without the per-block wait: 912 s, 1.39 GB resident, **8.7 GB** peak footprint); other builds were running on the machine |
| SPAN 2x, 960 × 720 → 1920 × 1440 | 9 – 11 s |

## Limits and open points

- **Memory**: the resident set stays near 1.3 GB, but Metal buffers count in the "peak memory
  footprint" (3.4 GB with the wait after each block, 8.7 GB without it): plan for a few GB of
  unified memory. `LIGHTCRAFT_RESTORE_TILE=256` trades speed for less.
- **Slow**: candle's Metal kernels are generic (a 3 × 3 convolution 64 → 64 at 512 × 512 runs at
  ~130 GFLOPS, the time is spread over convolutions, attention and layout copies; f16 was tried
  and is slower). A hand-written NHWC / fused kernel set, or a smaller network, is the way to a
  usable preview; this is a batch-export feature today.
- The step runs **after** the develop pipeline's sharpening and local contrast. Running it on the
  scene-linear image before them (the better place) needs a hook in the pipeline crate, which
  stays free of candle; not done.
- Exports only: no live preview, no progress in the UI (tiles are logged at `info`), no cancel
  from the app.
- The loaded network stays in memory for the next export of the session (about 0.1 GB).
- SCUNet has no noise-level input; very strong noise or JPEG artefacts can leave blotches, and
  the "real PSNR" variant prefers smooth to sharp. The "real GAN" variant of the same release
  (`scunet_color_real_gan.pth`) would keep more texture but invents it; not offered.
- Lens Blur is not part of this work.
