//! The model files, where they come from and under which terms. **None of them is part of
//! LightCraft**: they are downloaded only when the user asks, after a dialog naming the licence.

pub use lightcraft_fetch::FileSpec;

#[derive(Clone, Debug)]
pub struct Model {
    pub id: &'static str,
    pub title: &'static str,
    /// Sub-folder of the models folder.
    pub dir: &'static str,
    pub files: &'static [FileSpec<'static>],
    /// Base URL: each file is at `<mirror>/<file.name>`.
    pub mirror: &'static str,
    pub licence: &'static str,
    pub licence_url: &'static str,
    pub source_url: &'static str,
}

/// SegFormer-B2 fine-tuned on ADE20K (512 x 512), NVIDIA.
pub const SEGFORMER: Model = Model {
    id: "segformer",
    title: "SegFormer-B2 ADE20K (sky, people, landscape)",
    dir: "segformer-b2-ade",
    files: &[
        FileSpec {
            name: "pytorch_model.bin",
            size: Some(110_000_905),
            sha256: Some("187ca07bea003a5717c63d04ea90b07f33cd033c0ebf44b4b89fce5070d6c8f3"),
            max: 110_000_905,
        },
        FileSpec { name: "config.json", size: None, sha256: None, max: 65_536 },
    ],
    mirror: "https://huggingface.co/nvidia/segformer-b2-finetuned-ade-512-512/resolve/main",
    licence: "NVIDIA Source Code License (non-commercial research and evaluation); personal use only",
    licence_url: "https://huggingface.co/nvidia/segformer-b2-finetuned-ade-512-512",
    source_url: "https://github.com/NVlabs/SegFormer",
};

/// RMBG-1.4 (ISNet trained for general background removal), BRIA AI.
pub const RMBG: Model = Model {
    id: "subject",
    title: "RMBG-1.4 (subject matting)",
    dir: "rmbg-1.4",
    files: &[FileSpec {
        name: "model.safetensors",
        size: Some(176_381_984),
        sha256: Some("46ef7fe46f2ae284d8f1aaa24bfa5fca5ef25a34e2c7caa890a0029eb100e87f"),
        max: 176_381_984,
    }],
    mirror: "https://huggingface.co/briaai/RMBG-1.4/resolve/main",
    licence: "BRIA RMBG-1.4 licence (source-available, non-commercial use only); personal use only",
    licence_url: "https://bria.ai/bria-huggingface-model-license-agreement/",
    source_url: "https://huggingface.co/briaai/RMBG-1.4",
};

pub const ALL: [&Model; 2] = [&SEGFORMER, &RMBG];
