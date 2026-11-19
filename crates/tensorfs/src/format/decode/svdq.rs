//! `svdq.nvfp4` (spec §6 `svdq.nvfp4`): SVDQuant's weight, Nunchaku's svdq-fp4 in natural layout.
//! An nvfp4 residual of the smoothed weight, optionally a scale per row, and a 16-bit low-rank
//! branch.

use super::grid::Grid;
use super::mxfp8::blocks;
use super::rows::Follows;
use super::nvfp4::{self, PARAMS};
use super::plan::{Elems, Post};
use super::{logical, one, Ctx, Dtypes, Part, Plan, Scheme, Shape, View};
use crate::format::Result;

const FACTORS: Dtypes = &["BF16", "F16", "F32"];

pub(crate) static SVDQ_NVFP4: Scheme = Scheme {
    name: "svdq.nvfp4",
    view: View::Matrix,
    params: PARAMS,
    parts: &[
        Part::req("weight", &["F4"], matrix).rows(Follows::Logical),
        Part::req("scale", &["F8_E4M3"], nvfp4::scale).rows(Follows::Grid(blocks)),
        Part::req("scale_2", &["F32"], one).rows(Follows::Whole),
        Part::opt("input_scale", &["F32"], one).rows(Follows::Whole),
        Part::req("pre_quant_scale", &["F32", "BF16", "F16"], |c| Grid::Col.shape(c)).rows(Follows::Whole),
        Part::opt("channel_scale", &["F32", "F8_E4M3"], |c| Grid::Row.shape(c)).rows(Follows::Grid(|_| Grid::Row)),
        Part::req("lowrank_down", FACTORS, |c| Ok(Shape::Exact(vec![rank(c)?, c.cols as u64]))).rows(Follows::Whole),
        Part::req("lowrank_up", FACTORS, |c| Ok(Shape::Exact(vec![c.rows as u64, rank(c)?]))).rows(Follows::Logical),
    ],
    decode,
};

/// The logical `[N, K]`: a stack is refused.
fn matrix(c: &Ctx) -> Result<Shape> {
    match c.shape.len() {
        2 => logical(c),
        _ => Err(c.fail(format!("logical shape {:?} is not [N, K]", c.shape))),
    }
}

/// `R`, the rank `lowrank_down` `[R, K]` states: at least 1.
fn rank(c: &Ctx) -> Result<u64> {
    match c.part("lowrank_down").shape {
        [r, _] if *r > 0 => Ok(*r),
        s => Err(c.fail(format!("part lowrank_down has shape {s:?}, not [R, {}], R ≥ 1", c.cols))),
    }
}

/// nvfp4's `(T[q] × s) × g` (or its `divide` form), `× c[i]` where there is a `channel_scale`,
/// `× p[j]`, then `+ up[i, r] × down[r, j]` for each `r` in order.
fn decode<'a>(c: &Ctx<'a>) -> Result<Plan<'a>> {
    let part = |role| Elems::part(c.part(role));
    Ok(Plan {
        core: nvfp4::decode(c)?.core,
        post: Post::LowRank {
            rows: c.has("channel_scale").then(|| part("channel_scale")),
            cols: part("pre_quant_scale"),
            down: part("lowrank_down"),
            up: part("lowrank_up"),
        },
    })
}
