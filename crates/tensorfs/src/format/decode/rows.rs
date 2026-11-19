//! A stored tensor's rows (tfs-303): which bytes of each part hold the logical tensor's rows
//! `[start, stop)` along its first dimension, read without decoding. Whether and where a tensor may
//! be cut is the consuming op's to say; how finely its storage allows is its encoding's, declared
//! beside each part of its scheme (`Part::rows`). A part its scheme does not declare does not cut.

use std::collections::BTreeMap;

use super::grid::Grid;
use super::{Ctx, PartMetadata, Scheme};
use crate::format::{Body, Dtype, Encoding, Error, Result, Tensor, Value};

/// How a part's stored bytes follow the logical tensor's rows, as its scheme declares it.
#[derive(Clone, Copy)]
pub enum Follows {
    /// Undeclared: the tensor does not cut.
    Unknown,
    /// The same bytes for every range: a scalar, a per-column vector, a codebook.
    Whole,
    /// One element for the tensor (read whole), or one per matrix of a stack (cut with its rows).
    PerMatrix,
    /// The logical elements in stored order, rows first.
    Logical,
    /// On the scheme's scale grid.
    Grid(fn(&Ctx) -> Grid),
}

/// How one part is cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cut {
    /// Every range reads it whole.
    Whole,
    /// `bytes` for each run of `rows` rows from row 0; the last run may be short of rows (padded), never of bytes.
    Tiles { rows: u64, bytes: u64 },
}

/// One stored part of a tensor that cuts: its role (`""`: a plain tensor's data), length and cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowPart {
    pub role: String,
    pub length: u64,
    pub cut: Cut,
}

/// A stored tensor's rows: `count` along its first dimension; a range starts on a multiple of
/// `tile` and stops on one or at `count`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows {
    pub count: u64,
    pub tile: u64,
    pub parts: Vec<RowPart>,
}

/// One part's bytes of a range: `len` from `offset` of the part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span<'a> {
    pub role: &'a str,
    pub offset: u64,
    pub len: u64,
}

impl Rows {
    /// Each part's bytes for rows `[start, stop)`, in part order.
    pub fn range(&self, start: u64, stop: u64) -> Result<Vec<Span<'_>>> {
        let (tile, count) = (self.tile, self.count);
        if start >= stop || stop > count || start % tile != 0 || (stop % tile != 0 && stop != count) {
            return Err(Error::Invalid(format!(
                "rows {start}..{stop} of {count}: a range starts on a multiple of {tile} and stops on one or at {count}"
            )));
        }
        Ok(self.parts.iter().map(|p| match p.cut {
            Cut::Whole => Span { role: &p.role, offset: 0, len: p.length },
            Cut::Tiles { rows, bytes } => {
                let (first, end) = (start / rows, stop.div_ceil(rows));
                Span { role: &p.role, offset: first * bytes, len: (end - first) * bytes }
            }
        }).collect())
    }
}

impl Tensor {
    /// Its rows, or None where it does not cut: a scalar, an empty tensor, or a part its scheme
    /// does not declare.
    pub fn rows(&self) -> Result<Option<Rows>> {
        match &self.body {
            Body::Data(at) => {
                let Some(&count) = self.shape.first().filter(|n| **n > 0) else { return Ok(None) };
                Ok(per_row(at.length, count).map(|cut| Rows {
                    count,
                    tile: 1,
                    parts: vec![RowPart { role: String::new(), length: at.length, cut }],
                }))
            }
            Body::Encoded { encoding, parts } => {
                let parts: Vec<PartMetadata> = parts.iter().map(|p| PartMetadata {
                    role: &p.role,
                    dtype: &p.dtype,
                    shape: &p.shape,
                    len: usize::try_from(p.length).unwrap_or(usize::MAX),
                }).collect();
                rows(&self.dtype, &self.shape, encoding, &parts)
            }
        }
    }
}

/// An encoded tensor's rows from its scheme's declarations, its params and parts checked first.
pub fn rows(dtype: &Dtype, shape: &[u64], encoding: &Encoding, parts: &[PartMetadata<'_>]) -> Result<Option<Rows>> {
    let lengths: BTreeMap<&str, u64> = parts.iter().map(|p| (p.role, p.len as u64)).collect();
    super::with_ctx(dtype, shape, encoding, parts, |scheme, c| Ok(cut(scheme, c, &lengths)))
}

fn cut(scheme: &Scheme, c: &Ctx, lengths: &BTreeMap<&str, u64>) -> Option<Rows> {
    let transposed = matches!(c.params.get("transposed"), Some(Value::Bool(true)));
    let rank = c.shape.len();
    // A transposed matrix stores its columns first: no row of it is a contiguous run.
    let count = *c.shape.first().filter(|n| **n > 0 && !(transposed && rank == 2))?;
    let mut parts = Vec::new();
    for rule in scheme.parts {
        let (Some(p), Some(&length)) = (c.parts.get(rule.role), lengths.get(rule.role)) else { continue };
        let tiles = |rows: u64| {
            let runs = count.div_ceil(rows);
            length.is_multiple_of(runs).then(|| Cut::Tiles { rows, bytes: length / runs })
        };
        let cut = match rule.rows {
            Follows::Unknown => None,
            Follows::Whole => Some(Cut::Whole),
            Follows::PerMatrix if p.shape.iter().product::<u64>() == 1 => Some(Cut::Whole),
            Follows::PerMatrix if rank >= 3 => per_row(length, count),
            Follows::PerMatrix => None,
            Follows::Logical if p.shape.first() == Some(&count) => per_row(length, count),
            Follows::Logical => None,
            Follows::Grid(grid) => match grid(c) {
                Grid::Tensor | Grid::Col => Some(Cut::Whole),
                Grid::Row => per_row(length, count),
                // Each matrix of a stack is laid out on its own: whole matrices cut row by row.
                Grid::Blocks { layout, .. } => match layout.row_tile() {
                    Some(rows) if rank == 2 => tiles(rows),
                    Some(_) => per_row(length, count),
                    None => None,
                },
                Grid::Tiles { rows, .. } if rank == 2 => tiles(rows as u64),
                Grid::Tiles { .. } => per_row(length, count),
            },
        }?;
        parts.push(RowPart { role: rule.role.to_owned(), length, cut });
    }
    let tile = parts.iter().fold(1, |tile, p| match p.cut {
        Cut::Tiles { rows, .. } => lcm(tile, rows),
        Cut::Whole => tile,
    });
    Some(Rows { count, tile, parts })
}

/// Each row its own run of `length / count` bytes.
fn per_row(length: u64, count: u64) -> Option<Cut> {
    length.is_multiple_of(count).then(|| Cut::Tiles { rows: 1, bytes: length / count })
}

fn lcm(a: u64, b: u64) -> u64 {
    let (mut x, mut y) = (a, b);
    while y != 0 {
        (x, y) = (y, x % y);
    }
    a / x * b
}
