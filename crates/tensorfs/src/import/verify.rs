//! A planned import checked against its family ([`crate::family`]) before its bulk lands: each
//! base component's keys and shapes against the family's forms, the tensors a converter permutes
//! by the layout probe, and each LoRA component's targets against the base component it fits.
//!
//! A source a family's converter table wrote is refused when it matches no form or its probe
//! fails; that is a wrong conversion. A source in its own Diffusers layout whose class names a
//! family only gets no `family` then: it is some other model of that class.

use std::collections::BTreeMap;

use crate::err::{refuse, Code, Result};
use crate::family::{self, probe, Adapter, Family, Record, Stored, TrainedOnRecord};
use crate::format::{decode, Dtype, PartBytes};

use super::engine::{Planned, Reading, Recognized, Table};
use super::source::{Group, SourceSet};
use super::Layout;

/// What the check found: the base's family, each LoRA component's adapter, and what it said.
#[derive(Debug, Default)]
pub struct Checked {
    pub family: Option<Record>,
    pub adapters: BTreeMap<String, Adapter>,
    /// One line per finding (the form, each probed component's variant and cosine, why none).
    pub notes: Vec<String>,
}

const FACTORS: [&str; 3] = [".lora_A.weight", ".lora_B.weight", ".alpha"];

fn is_lora(tensors: &[&Planned]) -> bool {
    !tensors.is_empty() && tensors.iter().all(|p| FACTORS.iter().any(|f| p.key.ends_with(f)))
}

pub struct Input<'a> {
    pub set: &'a SourceSet,
    pub tables: &'a [Table],
    pub recognized: &'a Recognized,
    pub planned: &'a [Planned],
    pub layout: &'a Layout,
    /// Each output component's source group.
    pub sources: &'a BTreeMap<String, &'a Group>,
    /// A GGUF is converted by its own names table: no family is verified.
    pub gguf: bool,
    /// `--trained-on`: the component a LoRA was trained on, as its uploader says.
    pub trained_on: Option<&'a str>,
}

pub fn check(input: &Input) -> Result<Checked> {
    let mut by: BTreeMap<&str, Vec<&Planned>> = BTreeMap::new();
    for p in input.planned {
        by.entry(p.component.as_str()).or_default().push(p);
    }
    let (loras, bases): (Vec<_>, Vec<_>) = by.iter().partition(|(_, t)| is_lora(t));
    let mut out = Checked::default();
    if !input.gguf && !bases.is_empty() {
        base(input, &bases.iter().map(|(c, t)| (**c, t.as_slice())).collect::<Vec<_>>(), &mut out)?;
    }
    for (component, tensors) in &loras {
        if let Some(adapter) = lora(input, component, tensors)? {
            out.adapters.insert(component.to_string(), adapter);
        }
    }
    if let Some(asked) = input.trained_on.filter(|_| out.adapters.is_empty()) {
        return refuse(
            Code::UNKNOWN_FIELD,
            format!("--trained-on {asked} names what a LoRA was trained on; this source has no LoRA of a known family"),
        );
    }
    Ok(out)
}

fn table_names<'a>(input: &'a Input, lora: bool) -> Vec<&'a str> {
    (input.recognized.tables.iter())
        .map(|t| &input.tables[*t])
        .filter(|t| t.lora.is_some() == lora)
        .map(|t| t.name.as_str())
        .collect()
}

fn base(input: &Input, bases: &[(&str, &[&Planned])], out: &mut Checked) -> Result<()> {
    let tables = table_names(input, false);
    let by_table = tables.iter().find_map(|t| family::of_table(t));
    let classes = (input.layout.pipeline.iter().map(|p| &p.class.name))
        .chain(input.layout.components.iter().map(|c| &c.class.name));
    let Some(fam) = by_table.or_else(|| classes.into_iter().find_map(|c| family::of_class(c))) else {
        return Ok(());
    };
    let converted = by_table.is_some();
    let wrong = |code, why: String| match converted {
        true => refuse(code, format!(
            "{} converts it, but {why}. Name the format its producer writes (--source-format <format>), or keep \
             it unconverted (--source-format {})",
            tables.join(", "),
            super::AS_IS
        )),
        false => Ok(Some(format!("no family: {why}"))),
    };
    // Its components are verified; another component (an extra model in the repo) is not a wrong
    // conversion of the family, but the checkpoint then is not the family's alone.
    let (mine, foreign): (Vec<_>, Vec<_>) = bases.iter().partition(|(c, _)| fam.def(c).is_some());
    if mine.is_empty() {
        return Ok(());
    }
    let stored: Vec<Stored> = (mine.iter())
        .map(|(c, t)| Stored { name: c, keys: t.iter().map(|p| (p.key.as_str(), p.shape.as_slice())).collect() })
        .collect();
    let form = match fam.form_of(&stored) {
        Ok(form) => form,
        Err(why) => {
            out.notes.extend(wrong(Code::FAMILY_MISMATCH, format!("it is no {} form: {why}", fam.name))?);
            return Ok(());
        }
    };
    let mut components = BTreeMap::new();
    for (name, tensors) in &mine {
        match probe_component(input.set, fam, name, tensors)? {
            Probed::Unprobed => drop(components.insert(name.to_string(), name.to_string())),
            Probed::Matched { variant, line } => {
                out.notes.push(line);
                components.insert(name.to_string(), variant);
            }
            Probed::Unread(why) => {
                out.notes.push(format!("no family: {why}"));
                return Ok(());
            }
            Probed::Mismatch(why) => {
                out.notes.extend(wrong(Code::LAYOUT_MISMATCH, why)?);
                return Ok(());
            }
        }
    }
    if !foreign.is_empty() {
        let names: Vec<&str> = foreign.iter().map(|(c, _)| *c).collect();
        out.notes.push(format!("no family: {} {} not {}'s", names.join(", "), if names.len() == 1 { "is" } else { "are" }, fam.name));
        return Ok(());
    }
    out.notes.insert(0, format!("{} {form}", fam.name));
    out.family = Some(Record { name: fam.name.clone(), form, reference: fam.reference.clone(), components });
    Ok(())
}

enum Probed {
    /// The family probes none of the component's tensors.
    Unprobed,
    Matched { variant: String, line: String },
    /// Its probed tensors could not be read (an encoding this build cannot decode).
    Unread(String),
    Mismatch(String),
}

/// Which variant of its family component `name`'s probed tensors match, by the worst block of
/// each against each variant's reference sketches.
fn probe_component(set: &SourceSet, fam: &Family, name: &str, tensors: &[&Planned]) -> Result<Probed> {
    let (Some(probe), Some((root, _))) = (&fam.probe, fam.def(name)) else { return Ok(Probed::Unprobed) };
    let variants = fam.variants(name);
    // Every probed row range at once, read ahead in parallel.
    let mut ranges = Vec::new();
    for t in probe.of(root) {
        let Some(p) = tensors.iter().find(|p| p.key == t.key) else { continue };
        for b in &t.blocks {
            ranges.extend(block_ranges(p, b * probe.rows, probe.rows));
        }
    }
    set.read_ranges(&ranges)?;
    // Each variant's worst block over every probed tensor (cosine, key, block), and its mean
    // distance over them.
    let mut seen: BTreeMap<String, ((f64, String, u64), f64)> = BTreeMap::new();
    let mut probed = 0;
    for t in probe.of(root) {
        let Some(p) = tensors.iter().find(|p| p.key == t.key) else { continue };
        let cols: u64 = p.shape[1..].iter().product();
        let mut sketches = Vec::new();
        for b in &t.blocks {
            let rows = match block(set, p, b * probe.rows, probe.rows)? {
                Some(rows) => rows,
                None => return Ok(Probed::Unread(format!("{name}/{}: a {} tensor this build cannot read", p.key, scheme(p)))),
            };
            sketches.push(probe.sketch(&rows, cols as usize));
        }
        probed += 1;
        for a in probe::agreements(probe, t, &sketches).into_iter().filter(|a| variants.contains(&a.variant.as_str())) {
            let entry = seen.entry(a.variant.clone()).or_insert(((f64::INFINITY, String::new(), 0), 0.0));
            if a.worst < entry.0 .0 {
                entry.0 = (a.worst, t.key.clone(), a.worst_block);
            }
            entry.1 += a.distance;
        }
    }
    for entry in seen.values_mut() {
        entry.1 /= probed.max(1) as f64;
    }
    let Some((_, ((cosine, key, block), _))) = seen.iter().max_by(|a, b| a.1 .0 .0.total_cmp(&b.1 .0 .0)) else {
        return Ok(Probed::Unprobed);
    };
    if *cosine < probe.min_cosine {
        return Ok(Probed::Mismatch(format!(
            "{name}/{key} block {block} is not where {}'s reference holds its rows (layout probe cosine {cosine:.3}; \
             a fine-tune of it reads at least {}): the converter's layout is not this file's",
            fam.name, probe.min_cosine
        )));
    }
    // The variant it is: the nearest by mean distance, when distinctly nearer than every other.
    let (nearest, near) = seen.iter().min_by(|a, b| a.1 .1.total_cmp(&b.1 .1)).map(|(v, s)| (v, s.1)).expect("one seen");
    let distinct = seen.iter().all(|(v, s)| v == nearest || s.1 >= near * probe.distinct);
    let variant = if distinct { nearest.clone() } else { name.to_owned() };
    let shown: Vec<String> = seen.iter().map(|(v, s)| format!("{v} worst {:.4} mean distance {:.1e}", s.0 .0, s.1)).collect();
    let line = format!(
        "{name} is {} by its layout probe ({})",
        if distinct { variant.as_str() } else { "no one variant" },
        shown.join("; ")
    );
    Ok(Probed::Matched { variant, line })
}

fn scheme(p: &Planned) -> String {
    p.encoding.as_ref().map_or_else(|| p.dtype.to_string(), |e| e.scheme.clone())
}

/// `count` rows of `p` from row `first`, as f64 (`count × cols`); none for a tensor this build
/// cannot decode. A plain tensor reads only those rows; an encoded one is read whole and decoded.
fn block(set: &SourceSet, p: &Planned, first: u64, count: u64) -> Result<Option<Vec<f64>>> {
    let cols: u64 = p.shape[1..].iter().product();
    let Some(encoding) = &p.encoding else {
        let read = &p.parts[0].read;
        let Some(bits) = read.source.bits().filter(|b| b.is_multiple_of(8)) else { return Ok(None) };
        let bytes = match read.inline.is_some() || read.derive.is_some() {
            true => {
                let whole = whole(set, read)?;
                let row = (cols * bits / 8) as usize;
                whole[first as usize * row..(first + count) as usize * row].to_vec()
            }
            false => rows(set, read, bits / 8, cols, first, count)?,
        };
        return Ok(values(&read.source, &bytes));
    };
    let parts: Vec<(String, Vec<u8>)> =
        (p.parts.iter()).map(|part| Ok((part.role.clone(), whole(set, &part.read)?))).collect::<Result<_>>()?;
    let map: BTreeMap<&str, PartBytes> = (p.parts.iter().zip(&parts))
        .map(|(part, (role, bytes))| (role.as_str(), PartBytes { dtype: &part.dtype, shape: &part.shape, bytes }))
        .collect();
    let Ok(decoded) = decode(&p.dtype, &p.shape, encoding, &map) else { return Ok(None) };
    let Some(bits) = p.dtype.bits() else { return Ok(None) };
    let row = (cols * bits / 8) as usize;
    Ok(values(&p.dtype, &decoded[first as usize * row..(first + count) as usize * row]))
}

/// The ranges [`block`] reads of `p`: `(file, start, len)`.
fn block_ranges(p: &Planned, first: u64, count: u64) -> Vec<(usize, u64, u64)> {
    let cols: u64 = p.shape[1..].iter().product();
    let whole = |r: &Reading| ranges(r, r.unit, 1, 0, r.runs.count() * r.runs.run);
    let plain = |r: &Reading| match (r.inline.is_some() || r.derive.is_some(), r.source.bits()) {
        (false, Some(bits)) if bits.is_multiple_of(8) => ranges(r, bits / 8, cols, first, count),
        _ => vec![],
    };
    let out: Vec<(u64, u64, usize)> = match &p.encoding {
        None => plain(&p.parts[0].read).into_iter().map(|(s, l)| (s, l, p.parts[0].read.file)).collect(),
        Some(_) => (p.parts.iter()).flat_map(|part| whole(&part.read).into_iter().map(|(s, l)| (s, l, part.read.file))).collect(),
    };
    out.into_iter().map(|(s, l, f)| (f, s, l)).collect()
}

/// The byte ranges of output rows `first..first + count` of a plain reading (`elem` bytes per
/// element, `cols` per row), through its runs; adjacent ones joined.
fn ranges(read: &Reading, elem: u64, cols: u64, first: u64, count: u64) -> Vec<(u64, u64)> {
    let per = elem / read.unit;
    let (from, to) = (first * cols * per, (first + count) * cols * per);
    let run = read.runs.run;
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    let mut at = from;
    while at < to {
        let k = at / run;
        let end = to.min((k + 1) * run);
        let start = read.start + (read.runs.offset(k) + at - k * run) * read.unit;
        let len = (end - at) * read.unit;
        match ranges.last_mut() {
            Some(last) if last.0 + last.1 == start => last.1 += len,
            _ => ranges.push((start, len)),
        }
        at = end;
    }
    ranges
}

/// The source bytes of output rows `first..first + count` of a plain reading.
fn rows(set: &SourceSet, read: &Reading, elem: u64, cols: u64, first: u64, count: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (start, len) in ranges(read, elem, cols, first, count) {
        out.extend(set.read_at(read.file, start, len)?);
    }
    Ok(out)
}

/// A reading's whole bytes: inline, computed from its ranges, or through its runs.
fn whole(set: &SourceSet, read: &Reading) -> Result<Vec<u8>> {
    if let Some(bytes) = &read.inline {
        return Ok(bytes.clone());
    }
    if let Some(d) = &read.derive {
        let sources = (std::iter::once((read.start, read.len)).chain(d.with.iter().copied()))
            .map(|(start, len)| set.read_at(read.file, start, len))
            .collect::<Result<Vec<_>>>()?;
        return Ok((d.f)(&sources.iter().map(Vec::as_slice).collect::<Vec<_>>()));
    }
    let units = read.runs.count() * read.runs.run;
    rows(set, read, read.unit, 1, 0, units)
}

/// Little-endian elements of `dtype` as f64; none for a dtype the probe does not read.
fn values(dtype: &Dtype, bytes: &[u8]) -> Option<Vec<f64>> {
    use crate::format::decode::elem;
    let two = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
    Some(match dtype {
        Dtype::F32 => bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64).collect(),
        Dtype::BF16 => bytes.chunks_exact(2).map(|b| elem::bf16(two(b)) as f64).collect(),
        Dtype::F16 => bytes.chunks_exact(2).map(|b| elem::f16(two(b)) as f64).collect(),
        Dtype::F8_E4M3 => bytes.iter().map(|b| elem::e4m3(*b) as f64).collect(),
        Dtype::F8_E5M2 => bytes.iter().map(|b| elem::e5m2(*b) as f64).collect(),
        Dtype::I8 => bytes.iter().map(|b| *b as i8 as f64).collect(),
        _ => return None,
    })
}

/// The adapter a LoRA component is: the family and base component its factors fit, by its
/// converter table's family or, for a PEFT adapter, by the one family component every target
/// fits; and what it was trained on, when declared.
fn lora(input: &Input, component: &str, tensors: &[&Planned]) -> Result<Option<Adapter>> {
    let mut targets: BTreeMap<&str, (Option<&[u64]>, Option<&[u64]>)> = BTreeMap::new();
    for p in tensors {
        if let Some(path) = p.key.strip_suffix(".lora_A.weight") {
            targets.entry(path).or_default().0 = Some(&p.shape);
        } else if let Some(path) = p.key.strip_suffix(".lora_B.weight") {
            targets.entry(path).or_default().1 = Some(&p.shape);
        }
    }
    // A target fits base `[out, in, kernel…]` as A `[r, in, kernel…]` and B `[out, r, 1…]`.
    let fits = |fam: &Family, base: &str, path: &str, (a, b): &(Option<&[u64]>, Option<&[u64]>)| {
        let (Some(a), Some(b)) = (a, b) else { return false };
        let at = |v: &[Option<u64>], i: usize, got: u64| v.get(i).copied().flatten().is_none_or(|w| w == got);
        fam.targets(base, path).iter().any(|want| {
            want.len() == a.len() && a.len() == b.len() && a.len() >= 2 && at(want, 0, b[0]) && (1..a.len()).all(|i| at(want, i, a[i]))
        })
    };
    let claimed = (input.recognized.claims.iter())
        .find(|c| c.name == component && input.tables[c.table].lora.is_some())
        .and_then(|c| family::of_table(&input.tables[c.table].name));
    let (fam, base) = match claimed {
        Some(fam) => {
            let Some((root, _)) = fam.def(component) else { return Ok(None) };
            if let Some((path, _)) = targets.iter().find(|(path, t)| !fits(fam, root, path, t)) {
                return refuse(
                    Code::ADAPTER_TARGET_UNSUPPORTED,
                    format!("{component}/{path}: {} {root} has no such layer, or its factors do not fit it", fam.name),
                );
            }
            (fam, root.to_owned())
        }
        None => {
            let fitting: Vec<(&Family, &str)> = (family::families().iter())
                .flat_map(|f| f.components.iter().filter(|(_, d)| d.same_as.is_none()).map(move |(c, _)| (f, c.as_str())))
                .filter(|(f, c)| !targets.is_empty() && targets.iter().all(|(path, t)| fits(f, c, path, t)))
                .collect();
            match fitting.as_slice() {
                [(fam, base)] => (*fam, base.to_string()),
                _ => return Ok(None),
            }
        }
    };
    let trained_on = match input.trained_on {
        Some(asked) => {
            let variants = fam.variants(&base);
            if !variants.contains(&asked) {
                return refuse(
                    Code::UNKNOWN_FIELD,
                    format!("--trained-on {asked}: a {} {base} LoRA trains one of {}", fam.name, variants.join(", ")),
                );
            }
            Some(TrainedOnRecord { component: asked.to_owned(), from: "--trained-on".into() })
        }
        None => (input.sources.get(component))
            .and_then(|g| fam.trained_on(&g.metadata))
            .map(|(component, from)| TrainedOnRecord { component, from }),
    };
    Ok(Some(Adapter { family: fam.name.clone(), component: base, trained_on }))
}
