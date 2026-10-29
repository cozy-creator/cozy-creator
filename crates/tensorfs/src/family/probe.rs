//! The layout probe: whether a tensor a converter permutes holds its rows where the family's
//! reference does. A (key, shape) check cannot see a same-shape row permutation: a ComfyUI H3 DiT
//! read as the official layout has every key and shape right, and Q/K/V scrambled.
//!
//! The family keeps, per probed tensor, a sketch of some of the reference's `rows`-row blocks:
//! block `b`'s rows `W_b` give `y = W_b · r` (`r` the ±1 column signs of [`signs`] at `seed`),
//! and its sketch is `P · y` (`P` the `dims × rows` ±1 signs at `seed + 1`). A fine-tune of the
//! right layout keeps each block's direction; a block holding other rows does not. Ingest
//! computes the same sketch of the converted tensor and compares by cosine, block by block, with
//! each variant of the component (H3's FL2VA and Ref2VA transformers). Measured on H3 (dims 32):
//! fine-tunes of either variant agree ≥ 0.998 on every block, other blocks' rows ≤ 0.54, and
//! the variant a fine-tune derives from is 6–10× nearer than the other in mean distance.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub seed: u64,
    pub rows: u64,
    pub dims: usize,
    /// A block below this cosine with every variant is refused.
    pub min_cosine: f64,
    /// A variant is recorded when its mean distance (1 − cosine) is this many times below every
    /// other variant's; nearer than that, the component keeps its own name.
    pub distinct: f64,
    pub tensors: Vec<Tensor>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tensor {
    /// The canonical component and key.
    pub component: String,
    pub key: String,
    pub blocks: Vec<u64>,
    /// Each variant's sketches, `blocks × dims` little-endian F16, base64.
    pub sketch: BTreeMap<String, String>,
}

/// splitmix64's output for state `x`.
pub fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `n` signs at `seed`: element `j` is −1 when the top bit of `splitmix64(seed·2³² + j)` is set.
pub fn signs(seed: u64, n: usize) -> Vec<f64> {
    (0..n as u64)
        .map(|j| if splitmix64((seed << 32).wrapping_add(j)) >> 63 == 1 { -1.0 } else { 1.0 })
        .collect()
}

impl Probe {
    /// The sketch of one block, its rows row-major (`rows × cols` values).
    pub fn sketch(&self, block: &[f64], cols: usize) -> Vec<f64> {
        let r = signs(self.seed, cols);
        let y: Vec<f64> = block.chunks_exact(cols).map(|row| row.iter().zip(&r).map(|(w, s)| w * s).sum()).collect();
        let p = signs(self.seed + 1, self.dims * y.len());
        (p.chunks_exact(y.len())).map(|signs| signs.iter().zip(&y).map(|(s, v)| s * v).sum()).collect()
    }

    /// The probed tensors of a canonical component.
    pub fn of<'a>(&'a self, component: &'a str) -> impl Iterator<Item = &'a Tensor> + 'a {
        self.tensors.iter().filter(move |t| t.component == component)
    }
}

impl Tensor {
    /// A variant's sketches, one `dims`-vector per block.
    pub fn sketches(&self, variant: &str, dims: usize) -> Option<Vec<Vec<f64>>> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(self.sketch.get(variant)?).ok()?;
        let values: Vec<f64> = (bytes.chunks_exact(2))
            .map(|b| crate::format::decode::elem::f16(u16::from_le_bytes([b[0], b[1]])) as f64)
            .collect();
        (values.len() == dims * self.blocks.len()).then(|| values.chunks(dims).map(<[f64]>::to_vec).collect())
    }
}

pub fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f64]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
    match norm(a) * norm(b) {
        0.0 => 0.0,
        n => dot / n,
    }
}

/// One probed tensor's agreement with one variant: its worst block, that block's cosine, and the
/// mean distance (1 − cosine) over its blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct Agreement {
    pub variant: String,
    pub worst_block: u64,
    pub worst: f64,
    pub distance: f64,
}

/// Each variant's agreement with `sketches` (the converted tensor's, one per probed block).
pub fn agreements(probe: &Probe, tensor: &Tensor, sketches: &[Vec<f64>]) -> Vec<Agreement> {
    let mut out = Vec::new();
    for variant in tensor.sketch.keys() {
        let Some(reference) = tensor.sketches(variant, probe.dims) else { continue };
        let cosines: Vec<(u64, f64)> =
            (tensor.blocks.iter().zip(reference.iter().zip(sketches))).map(|(b, (r, s))| (*b, cosine(r, s))).collect();
        let Some(&(worst_block, worst)) = cosines.iter().min_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
        let distance = cosines.iter().map(|(_, c)| 1.0 - c).sum::<f64>() / cosines.len() as f64;
        out.push(Agreement { variant: variant.clone(), worst_block, worst, distance });
    }
    out
}
