#!/usr/bin/env python3
"""Optional external runner: SCUNet blind denoising on the Apple GPU with PyTorch (MPS).

LightCraft's own SCUNet (pure Rust, candle) is exact but candle's Metal convolutions reach ~160
GFLOP/s where PyTorch's MPS reaches ~2.6 TFLOP/s on the same M4; a 22 Mpx photo takes ~15 minutes
in Rust and about a minute here. LightCraft calls this script when it finds a Python with PyTorch
(`pip install torch numpy`) and falls back to the Rust implementation otherwise. Same weights
(`scunet_color_real_psnr.pth`, checked by SHA-256 by LightCraft), same tiling, same result.

The network is SCUNet (Zhang et al., "Practical Blind Image Denoising via Swin-Conv-UNet and Data
Synthesis", Machine Intelligence Research 2023), written here from the paper and the checkpoint's
layout (no code of the reference implementation is used). Original code, MIT OR Apache-2.0.

  scunet_torch.py --weights scunet_color_real_psnr.pth --width W --height H
                  [--tile 512] [--overlap 48] [--device mps|cpu] IN.f32 OUT.f32

IN/OUT: raw little-endian float32, interleaved RGB, H x W x 3, display-encoded 0..1.
Progress lines "tile i/n" go to stderr.
"""
import argparse
import os
import sys
import time

# Homebrew's torch and numpy can each bring an OpenMP runtime; the duplicate is harmless for this
# single-threaded CPU side (the work runs on the GPU) once told so.
os.environ.setdefault("KMP_DUPLICATE_LIB_OK", "TRUE")
os.environ.setdefault("OMP_NUM_THREADS", "1")

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F

WINDOW, HEAD_DIM, BLOCKS, DIM = 8, 32, 4, 64


class WMSA(nn.Module):
    def __init__(self, dim, shifted):
        super().__init__()
        self.heads, self.shifted = dim // HEAD_DIM, shifted
        self.embedding_layer = nn.Linear(dim, 3 * dim)
        self.relative_position_params = nn.Parameter(torch.zeros(self.heads, 2 * WINDOW - 1, 2 * WINDOW - 1))
        self.linear = nn.Linear(dim, dim)
        self._bias = None
        self._masks = {}

    def bias(self, dev):
        if self._bias is None or self._bias.device != dev:
            idx = torch.arange(WINDOW)
            cord = torch.stack(torch.meshgrid(idx, idx, indexing="ij"), -1).reshape(-1, 2)
            rel = cord[:, None, :] - cord[None, :, :] + WINDOW - 1
            self._bias = self.relative_position_params[:, rel[..., 0], rel[..., 1]].unsqueeze(0).to(dev)
        return self._bias

    def mask(self, nh, nw, dev):
        key = (nh, nw, str(dev))
        if key not in self._masks:
            n, s = WINDOW * WINDOW, WINDOW // 2
            q = torch.arange(n)
            r, c = q // WINDOW, q % WINDOW
            diff_r = (r[:, None] < s) != (r[None, :] < s)
            diff_c = (c[:, None] < s) != (c[None, :] < s)
            m = torch.zeros(nh, nw, n, n)
            m[nh - 1] = torch.where(diff_r, -1e9, 0.0)  # last window row: rows from different parts
            m[:, nw - 1] = m[:, nw - 1] + torch.where(diff_c, -1e9, 0.0)
            self._masks[key] = m.reshape(nh * nw, 1, n, n).clamp(min=-1e9).to(dev)
        return self._masks[key]

    def forward(self, x):  # x: [1, H, W, C]
        _, h, w, c = x.shape
        nh, nw = h // WINDOW, w // WINDOW
        if self.shifted:
            x = torch.roll(x, shifts=(-(WINDOW // 2), -(WINDOW // 2)), dims=(1, 2))
        win = x.reshape(1, nh, WINDOW, nw, WINDOW, c).permute(0, 1, 3, 2, 4, 5).reshape(nh * nw, WINDOW * WINDOW, c)
        qkv = self.embedding_layer(win).reshape(nh * nw, WINDOW * WINDOW, 3 * self.heads, HEAD_DIM)
        q, k, v = (qkv[:, :, i * self.heads:(i + 1) * self.heads].permute(0, 2, 1, 3) for i in range(3))
        sim = (q @ k.transpose(2, 3)) * HEAD_DIM ** -0.5 + self.bias(x.device)
        if self.shifted:
            sim = sim + self.mask(nh, nw, x.device)
        out = (sim.softmax(-1) @ v).permute(0, 2, 1, 3).reshape(nh * nw, WINDOW * WINDOW, c)
        out = self.linear(out).reshape(1, nh, nw, WINDOW, WINDOW, c).permute(0, 1, 3, 2, 4, 5).reshape(1, h, w, c)
        if self.shifted:
            out = torch.roll(out, shifts=(WINDOW // 2, WINDOW // 2), dims=(1, 2))
        return out


class SwinBlock(nn.Module):
    def __init__(self, dim, shifted):
        super().__init__()
        self.ln1, self.msa, self.ln2 = nn.LayerNorm(dim), WMSA(dim, shifted), nn.LayerNorm(dim)
        self.mlp = nn.Sequential(nn.Linear(dim, 4 * dim), nn.GELU(), nn.Linear(4 * dim, dim))

    def forward(self, x):
        x = x + self.msa(self.ln1(x))
        return x + self.mlp(self.ln2(x))


class ConvTransBlock(nn.Module):
    def __init__(self, half, shifted):
        super().__init__()
        self.cd = self.td = half
        c = 2 * half
        self.conv1_1, self.conv1_2 = nn.Conv2d(c, c, 1), nn.Conv2d(c, c, 1)
        self.trans_block = SwinBlock(half, shifted)
        self.conv_block = nn.Sequential(nn.Conv2d(half, half, 3, 1, 1, bias=False), nn.ReLU(True), nn.Conv2d(half, half, 3, 1, 1, bias=False))

    def forward(self, x):
        cx, tx = torch.split(self.conv1_1(x), (self.cd, self.td), dim=1)
        cx = self.conv_block(cx) + cx
        tx = self.trans_block(tx.permute(0, 2, 3, 1)).permute(0, 3, 1, 2)
        return x + self.conv1_2(torch.cat((cx, tx), dim=1))


def stage(half):
    return [ConvTransBlock(half, i % 2 == 1) for i in range(BLOCKS)]


class SCUNet(nn.Module):
    def __init__(self):
        super().__init__()
        d = DIM
        self.m_head = nn.Sequential(nn.Conv2d(3, d, 3, 1, 1, bias=False))
        self.m_down1 = nn.Sequential(*stage(d // 2), nn.Conv2d(d, 2 * d, 2, 2, 0, bias=False))
        self.m_down2 = nn.Sequential(*stage(d), nn.Conv2d(2 * d, 4 * d, 2, 2, 0, bias=False))
        self.m_down3 = nn.Sequential(*stage(2 * d), nn.Conv2d(4 * d, 8 * d, 2, 2, 0, bias=False))
        self.m_body = nn.Sequential(*stage(4 * d))
        self.m_up3 = nn.Sequential(nn.ConvTranspose2d(8 * d, 4 * d, 2, 2, 0, bias=False), *stage(2 * d))
        self.m_up2 = nn.Sequential(nn.ConvTranspose2d(4 * d, 2 * d, 2, 2, 0, bias=False), *stage(d))
        self.m_up1 = nn.Sequential(nn.ConvTranspose2d(2 * d, d, 2, 2, 0, bias=False), *stage(d // 2))
        self.m_tail = nn.Sequential(nn.Conv2d(d, 3, 3, 1, 1, bias=False))

    def forward(self, x0):
        x1 = self.m_head(x0)
        x2 = self.m_down1(x1)
        x3 = self.m_down2(x2)
        x4 = self.m_down3(x3)
        x = self.m_body(x4)
        x = self.m_up3(x + x4)
        x = self.m_up2(x + x3)
        x = self.m_up1(x + x2)
        return self.m_tail(x + x1)


def starts(n, tile, overlap):
    if n <= tile:
        return [0]
    stride, v, s = max(tile - overlap, 1), [], 0
    while s + tile < n:
        v.append(s)
        s += stride
    v.append(n - tile)
    return v


def ramp(n, overlap, before, after):
    i = np.arange(n, dtype=np.float32)
    a = np.minimum((i + 0.5) / overlap, 1.0) if before else np.ones(n, np.float32)
    b = np.minimum((n - i - 0.5) / overlap, 1.0) if after else np.ones(n, np.float32)
    return np.minimum(a, b)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--weights", required=True)
    ap.add_argument("--width", type=int, required=True)
    ap.add_argument("--height", type=int, required=True)
    ap.add_argument("--tile", type=int, default=512)
    ap.add_argument("--overlap", type=int, default=48)
    ap.add_argument("--device", default="mps")
    ap.add_argument("input")
    ap.add_argument("output")
    a = ap.parse_args()
    dev = torch.device(a.device if a.device != "mps" or torch.backends.mps.is_available() else "cpu")
    model = SCUNet()
    state = torch.load(a.weights, map_location="cpu", weights_only=True)
    state = {k: v for k, v in state.items() if "attn_mask" not in k and "relative_position_index" not in k}
    model.load_state_dict(state, strict=True)
    model.eval().to(dev)
    w, h = a.width, a.height
    img = np.fromfile(a.input, dtype="<f4").reshape(h, w, 3)
    tile = max(64, a.tile // 64 * 64)
    overlap = min(a.overlap, tile // 2)
    xs, ys = starts(w, tile, overlap), starts(h, tile, overlap)
    out = np.zeros((h, w, 3), np.float32)
    wsum = np.zeros((h, w), np.float32)
    total, done = len(xs) * len(ys), 0
    with torch.inference_mode():
        for y0 in ys:
            for x0 in xs:
                tw, th = min(tile, w - x0), min(tile, h - y0)
                pw, ph = -(-tw // 64) * 64, -(-th // 64) * 64
                crop = np.pad(img[y0:y0 + th, x0:x0 + tw], ((0, ph - th), (0, pw - tw), (0, 0)), mode="edge")
                x = torch.from_numpy(np.ascontiguousarray(crop.transpose(2, 0, 1)[None])).to(dev)
                y = model(x)[0, :, :th, :tw].permute(1, 2, 0).float().cpu().numpy()
                wt = ramp(th, overlap, y0 > 0, y0 + th < h)[:, None] * ramp(tw, overlap, x0 > 0, x0 + tw < w)[None, :]
                out[y0:y0 + th, x0:x0 + tw] += y * wt[..., None]
                wsum[y0:y0 + th, x0:x0 + tw] += wt
                done += 1
                print(f"tile {done}/{total}", file=sys.stderr, flush=True)
    (out / np.maximum(wsum, 1e-12)[..., None]).astype("<f4").tofile(a.output)


if __name__ == "__main__":
    t = time.time()
    main()
    print(f"done in {time.time() - t:.1f}s", file=sys.stderr)
