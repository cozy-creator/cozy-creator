//! project.yaml written in place: the values a write changes are spliced into the file's text, each
//! in the style around it (flow stays flow, block stays block), and every other byte (comments,
//! quoting, order) stays. People hand-edit the file; cozy's writes must not undo that.
use saphyr::{LoadableYamlNode, MarkedYaml, Marker, Scalar, ScalarOwned, Yaml, YamlData, YamlOwned};

/// `text`, whose document is `old`, with `new` written where the two differ; and the paths of the
/// changes a splice cannot place (an entity added or removed), which are left as `text` has them.
pub fn splice(text: &str, old: &YamlOwned, new: &YamlOwned) -> Result<(String, Vec<String>), String> {
    let docs = MarkedYaml::load_from_str(text).map_err(|e| e.to_string())?;
    let [marked] = docs.as_slice() else { return Err("project.yaml holds one YAML document".into()) };
    // A marker counts characters; a splice needs bytes.
    let at: Vec<usize> = text.char_indices().map(|(byte, _)| byte).chain([text.len()]).collect();
    let mut edits = Edits { text, at: &at, out: vec![], unplaced: vec![] };
    edits.node(new, old, marked, "");
    // Last first, so earlier offsets hold; of two at one place, the later one first, so they read in order.
    let mut out = text.to_owned();
    let mut ordered: Vec<_> = edits.out.into_iter().enumerate().collect();
    ordered.sort_by_key(|(n, (start, ..))| std::cmp::Reverse((*start, *n)));
    for (_, (start, end, with)) in ordered {
        out.replace_range(start..end, &with);
    }
    Ok((out, edits.unplaced))
}

struct Edits<'a> {
    text: &'a str,
    at: &'a [usize],
    /// Byte ranges and what replaces each.
    out: Vec<(usize, usize, String)>,
    unplaced: Vec<String>,
}

impl Edits<'_> {
    fn byte(&self, marker: Marker) -> usize {
        self.at[marker.index().min(self.at.len() - 1)]
    }

    fn flow(&self, node: &MarkedYaml) -> bool {
        matches!(self.text[self.byte(node.span.start)..].chars().next(), Some('{' | '['))
    }

    /// Where a flow collection's closing bracket is.
    fn closing(&self, node: &MarkedYaml) -> usize {
        let close = if matches!(node.data, YamlData::Mapping(_)) { '}' } else { ']' };
        let from = self.byte(node.span.end);
        from + self.text[from..].find(close).unwrap_or(self.text.len() - from)
    }

    /// The end of the line that holds the end of `node`'s last value (its comment included).
    fn line_end(&self, node: &MarkedYaml) -> usize {
        let last = match &node.data {
            YamlData::Mapping(map) => map.iter().next_back().map(|(_, value)| value),
            YamlData::Sequence(items) => items.last(),
            _ => None,
        };
        let from = match last {
            Some(last) => return self.line_end(last),
            None => self.byte(node.span.end),
        };
        from + self.text[from..].find('\n').unwrap_or(self.text.len() - from)
    }

    fn node(&mut self, new: &YamlOwned, old: &YamlOwned, at: &MarkedYaml, path: &str) {
        if new == old {
            return;
        }
        let col = at.span.start.col();
        match (new, old, &at.data) {
            (YamlOwned::Mapping(n), YamlOwned::Mapping(o), YamlData::Mapping(m)) => {
                for (key, value) in n {
                    let name = key.as_str().unwrap_or_default();
                    match (o.get(key), m.iter().find(|(k, _)| k.data.as_str() == key.as_str())) {
                        (Some(was), Some((_, marked))) => self.node(value, was, marked, &join(path, name)),
                        _ if self.flow(at) && o.is_empty() => {
                            let close = self.closing(at);
                            self.out.push((close, close, format!("{name}: {}", flow(value))));
                        }
                        _ if self.flow(at) => {
                            let end = self.byte(at.span.end);
                            self.out.push((end, end, format!(", {name}: {}", flow(value))));
                        }
                        _ => {
                            let end = self.line_end(at);
                            self.out.push((end, end, format!("\n{}{name}:{}", " ".repeat(col), block(value, col + 2))));
                        }
                    }
                }
                for key in o.keys().filter(|key| !n.contains_key(*key)) {
                    self.unplaced.push(join(path, key.as_str().unwrap_or_default()));
                }
            }
            (YamlOwned::Sequence(n), YamlOwned::Sequence(o), YamlData::Sequence(items))
                if !n.is_empty() && !o.is_empty() && n.iter().chain(o).all(|item| named(item).is_some()) =>
            {
                for item in n {
                    match o.iter().position(|was| named(was) == named(item)) {
                        Some(j) => self.node(item, &o[j], &items[j], &join(path, named(item).unwrap_or_default())),
                        None => self.unplaced.push(join(path, named(item).unwrap_or_default())),
                    }
                }
                for was in o.iter().filter(|was| !n.iter().any(|item| named(item) == named(was))) {
                    self.unplaced.push(join(path, named(was).unwrap_or_default()));
                }
            }
            (YamlOwned::Sequence(n), YamlOwned::Sequence(o), YamlData::Sequence(items))
                if !o.is_empty() && n.starts_with(o) =>
            {
                // Grown: the new items after the last one.
                let extra = &n[o.len()..];
                match self.flow(at) {
                    true => {
                        let close = self.closing(at);
                        self.out.push((close, close, extra.iter().map(|item| format!(", {}", flow(item))).collect()));
                    }
                    false => {
                        let end = self.line_end(&items[items.len() - 1]);
                        let lines = extra.iter().map(|item| format!("\n{}- {}", " ".repeat(col), flow(item)));
                        self.out.push((end, end, lines.collect()));
                    }
                }
            }
            _ => {
                // The value rewritten where it stands, in its style.
                let start = self.byte(at.span.start);
                let (end, with) = match (&at.data, self.flow(at)) {
                    (YamlData::Mapping(_) | YamlData::Sequence(_), true) => (self.closing(at) + 1, flow(new)),
                    (YamlData::Sequence(_), false) if matches!(new, YamlOwned::Sequence(n) if !n.is_empty()) => {
                        let YamlOwned::Sequence(items) = new else { unreachable!() };
                        let lines: Vec<String> = items.iter().map(|item| format!("- {}", flow(item))).collect();
                        (self.line_end(at), lines.join(&format!("\n{}", " ".repeat(col))))
                    }
                    (YamlData::Mapping(_) | YamlData::Sequence(_), false) => (self.line_end(at), flow(new)),
                    _ => (self.byte(at.span.end), flow(new)),
                };
                self.out.push((start, end, with));
            }
        }
    }
}

/// A value after a block mapping's `key:`: a scalar or empty list on the line, a list or mapping
/// on the lines below, indented to `col`.
fn block(value: &YamlOwned, col: usize) -> String {
    let pad = " ".repeat(col);
    match value {
        YamlOwned::Sequence(items) if !items.is_empty() => {
            items.iter().map(|item| format!("\n{pad}- {}", flow(item))).collect()
        }
        YamlOwned::Mapping(map) if !map.is_empty() => map
            .iter()
            .map(|(key, value)| format!("\n{pad}{}:{}", key.as_str().unwrap_or_default(), block(value, col + 2)))
            .collect(),
        value => format!(" {}", flow(value)),
    }
}

/// A value in flow style: `[a, b]`, `{k: v}`, or a scalar, quoted when plain would read otherwise.
fn flow(value: &YamlOwned) -> String {
    match value {
        YamlOwned::Sequence(items) => format!("[{}]", items.iter().map(flow).collect::<Vec<_>>().join(", ")),
        YamlOwned::Mapping(map) => {
            let pairs: Vec<String> =
                map.iter().map(|(k, v)| format!("{}: {}", k.as_str().unwrap_or_default(), flow(v))).collect();
            format!("{{{}}}", pairs.join(", "))
        }
        YamlOwned::Value(ScalarOwned::String(text)) => {
            let plain = !text.is_empty()
                && text.chars().all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c))
                && !text.starts_with(['-', '.'])
                && matches!(Yaml::value_from_str(text), Yaml::Value(Scalar::String(_)));
            match plain {
                true => text.clone(),
                false => serde_json::to_string(text).expect("a string serializes"),
            }
        }
        YamlOwned::Value(ScalarOwned::Integer(n)) => n.to_string(),
        YamlOwned::Value(ScalarOwned::FloatingPoint(f)) => f.to_string(),
        YamlOwned::Value(ScalarOwned::Boolean(b)) => b.to_string(),
        _ => "~".into(),
    }
}

/// A named mapping's name.
fn named(item: &YamlOwned) -> Option<&str> {
    item.as_mapping_get("name").and_then(YamlOwned::as_str)
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() { key.to_owned() } else { format!("{path}.{key}") }
}
