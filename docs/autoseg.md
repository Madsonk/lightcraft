# Automatic masks: sky, people, landscape, subject

`mask.auto` (and the Sky / Subject / Background tiles of the Masking panel once the models are installed)
select by what the picture shows, with two optional networks run in pure Rust on candle
(`crates/autoseg`, no C, no unsafe):

| Model | Used for | File | Size | Licence (as published) |
|---|---|---|---:|---|
| `segformer`: SegFormer-B2, ADE20K 150 classes (Xie et al. 2021) | sky, people, vegetation, water, mountain, buildings, ground | `pytorch_model.bin` | 110 000 905 | NVIDIA Source Code License: research / non-commercial. **Personal use only** |
| `subject`: RMBG-1.4, an ISNet / U²-Net-family matting network | subject, background | `model.safetensors` | 176 381 984 | BRIA RMBG-1.4: source-available, non-commercial. **Personal use only** |

The weights are not part of LightCraft: `autoseg.model.download {model, acknowledged: true}` fetches them
(after the user agreed to the licence above; size and SHA-256 checked, resumable), into
`models/autoseg/<model folder>` of the settings folder (`LIGHTCRAFT_AUTOSEG_DIR` to move it). The two
networks are written from their papers and the checkpoints' layouts; no GPL / AGPL / LGPL code is used.

```sh
lightcraft-cli run autoseg.model.status
lightcraft-cli run autoseg.model.download model=segformer acknowledged=true
lightcraft-cli run --library ~/Pictures/Test.lcl --import ~/Pictures/trip mask.auto kind=sky \
    mask.auto kind=people name=Models mask.auto kind=background
```

`mask.auto {kind: sky|people|subject|background|vegetation|water|mountain|buildings|ground, ids?, name?,
refine?}` adds a mask to the active photo (or each of `ids`): one `Describe`-type component whose
segmentation is **stored with the mask** (as SAM 3's are), so it renders, exports and syncs without the
model. `background` is the subject, inverted. `refine` (default 30) is the mask's *Refine Edges*: the
network's grid is coarser than the photo, the guided filter snaps its edges to the photo's. Everything
else about a mask (adjustments, invert, intersect with a gradient, brush corrections) works as usual.

`mask.add {kind: sky|subject|background}` uses the networks when they are installed (`classic: true`
keeps the previous heuristics); without them nothing changes.

Speed (M4, the CPU: candle's Metal is slower on these small-channel layers): see the measurements in
`infra-artcraft.md`. Limits: SegFormer's class grid is 1/4 of a 512 px short side, the stored mask is 512
squared, the subject mask 1024 squared: edges depend on *Refine Edges*; hair against sky needs a manual
touch. A subject network picks "the salient thing": a plate of food on a table is the plate, a crowd is
everyone.
