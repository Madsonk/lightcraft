//! `lightcraft-retouch`: the maths of LightCraft's proposed retouch (pure functions, no model, no
//! network, no pixel is generated: everything here measures pixels or draws soft planes that the
//! engine stores as ordinary editable masks).

pub mod dnb;
pub mod facegeo;
pub mod lab;
pub mod palette;
pub mod plane;
pub mod qc;
pub mod skin;
