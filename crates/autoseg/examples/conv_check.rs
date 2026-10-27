#![allow(clippy::unwrap_used)]
use candle_core::{Device, Tensor};
fn s(t: &Tensor) -> (f64, f64) {
    let v = t.flatten_all().unwrap().to_vec1::<f32>().unwrap();
    (v.iter().map(|a| f64::from(*a)).sum(), v.iter().map(|a| f64::from(*a).powi(2)).sum())
}
fn main() {
    let d = if std::env::args().nth(1).as_deref() == Some("metal") { Device::new_metal(0).unwrap() } else { Device::Cpu };
    for (c, hw) in [(32usize, 64usize), (32, 128), (64, 64), (8, 64)] {
        let n = c * hw * hw;
        let x: Vec<f32> = (0..n).map(|i| ((i * 37 % 1013) as f32) / 500.0 - 1.0).collect();
        let xt = Tensor::from_vec(x, (1, c, hw, hw), &d).unwrap();
        let p = xt.max_pool2d_with_stride(2, 2).unwrap();
        println!("pool {c}ch {hw}: {:?} {:?}", p.dims(), s(&p));
    }
    for (ci, co, hw) in
        [(32usize, 32usize, 32usize), (32, 32, 33), (16, 16, 32), (32, 32, 31), (32, 32, 34), (64, 64, 32), (32, 64, 32), (32, 32, 64), (64, 32, 16)]
    {
        let n = ci * hw * hw;
        let x: Vec<f32> = (0..n).map(|i| ((i * 37 % 1013) as f32) / 500.0 - 1.0).collect();
        let w: Vec<f32> = (0..co * ci * 9).map(|i| ((i * 53 % 891) as f32) / 4500.0 - 0.1).collect();
        let y = Tensor::from_vec(x, (1, ci, hw, hw), &d).unwrap().conv2d(&Tensor::from_vec(w, (co, ci, 3, 3), &d).unwrap(), 1, 1, 1, 1).unwrap();
        println!("conv {ci}->{co} @{hw}: {:?}", s(&y));
    }
}
