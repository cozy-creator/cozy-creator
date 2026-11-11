//! Catalog role, independent of a checkpoint's model family or component architecture.
use std::collections::BTreeMap;
use super::{Header, Value};

impl Header {
    /// Classifies readable stored tensors as a model or LoRA. Ordinary saved-head tensors
    /// do not hide an adapter. Empty headers have no known repository role.
    pub fn model_role(&self) -> &'static str {
        let components = self.components();
        if !components.iter().any(|component| !component.tensors.is_empty()) {
            return "unknown";
        }
        if let Some(Value::Map(adapters)) = self.metadata.get("adapter") {
            for component in components.iter().filter(|component| !component.name.is_empty() && !component.tensors.is_empty()) {
                let Some(Value::Map(adapter)) = adapters.get(&component.name) else { continue };
                let text = |key| matches!(adapter.get(key), Some(Value::Str(value)) if !value.trim().is_empty());
                if text("family") && text("component") { return "lora"; }
            }
        }
        for component in components {
            let mut pairs = BTreeMap::new();
            for tensor in &component.tensors {
                if let Some((module, adapter, side)) = factor(&tensor.key) {
                    let pair = pairs.entry((module, adapter)).or_insert(0u8);
                    *pair |= side;
                    if *pair == 3 { return "lora"; }
                }
            }
        }
        "model"
    }
}

/// PEFT A/B and Kohya down/up are equivalent factor names. A named adapter must match too.
fn factor(key: &str) -> Option<(&str, &str, u8)> {
    let key = key.strip_prefix("base_model.model.").unwrap_or(key);
    for (marker, side, named) in [(".lora_A", 1, true), (".lora_B", 2, true), (".lora_down", 1, false), (".lora_up", 2, false)] {
        let Some((module, suffix)) = key.rsplit_once(marker) else { continue };
        if module.is_empty() { continue; }
        let adapter = match suffix {
            "" | ".weight" => "",
            suffix if named => match suffix.strip_prefix('.').and_then(|suffix| suffix.strip_suffix(".weight")).filter(|name| !name.is_empty()) {
                Some(adapter) => adapter,
                None => continue,
            },
            _ => continue,
        };
        return Some((module, adapter, side));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Body, ClassRef, Component, Dtype, Location, ObjectRef};

    fn header(components: &[(&str, &[&str])], metadata: BTreeMap<String, Value>) -> Header {
        let components = components.iter().map(|(name, keys)| Component {
            name: (*name).into(),
            class: ClassRef::new("torch", "1", "Module"),
            tensors: keys.iter().map(|key| crate::format::Tensor {
                key: (*key).into(), dtype: Dtype::F32, shape: vec![1], body: Body::Data(Location { chunk: 0, offset: 0, length: 4 }),
            }).collect(),
            ..Default::default()
        }).collect();
        Header::new(vec![ObjectRef::of(&[0; 4])], None, components, metadata).unwrap()
    }

    #[test]
    fn model_role_matches_catalog_semantics() {
        for (label, keys, role) in [
            ("base_model", vec!["layer.weight"], "model"),
            ("standalone_component_model", vec!["embedding.weight"], "model"),
            ("canonical_lora", vec!["layer.lora_A.weight", "layer.lora_B.weight"], "lora"),
            ("peft_named_lora", vec!["base_model.model.layer.lora_A.default.weight", "base_model.model.layer.lora_B.default.weight"], "lora"),
            ("kohya_lora", vec!["layer.lora_down", "layer.lora_up"], "lora"),
            ("mixed_saved_head_lora", vec!["layer.lora_A.weight", "layer.lora_B.weight", "head.modules_to_save.default.weight"], "lora"),
            ("mixed_equivalent_aliases_lora", vec!["layer.lora_A.weight", "layer.lora_up.weight"], "lora"),
            ("unpaired_model", vec!["layer.lora_A.weight"], "model"),
            ("mismatched_adapter_names_model", vec!["layer.lora_A.first.weight", "layer.lora_B.second.weight"], "model"),
        ] {
            assert_eq!(header(&[("transformer", &keys)], Default::default()).model_role(), role, "{label}");
        }
        assert_eq!(Header::default().model_role(), "unknown", "empty_unknown");
        let cross = header(&[("first", &["layer.lora_A.weight"]), ("second", &["layer.lora_B.weight"])], Default::default());
        assert_eq!(cross.model_role(), "model", "cross_component_unpaired_model");
    }

    #[test]
    fn explicit_adapter_metadata_requires_a_populated_component() {
        let adapter = |value| BTreeMap::from([("adapter".into(), Value::Map(BTreeMap::from([("transformer".into(), value)])))]);
        let valid = Value::Map(BTreeMap::from([("family".into(), "sdxl".into()), ("component".into(), "unet".into())]));
        assert_eq!(header(&[("transformer", &["head.weight"])], adapter(valid.clone())).model_role(), "lora", "explicit_adapter_lora");
        assert_eq!(header(&[("transformer", &["head.weight"])], adapter("broken".into())).model_role(), "model", "malformed_adapter_model");
        assert_eq!(header(&[("transformer", &[]), ("other", &["head.weight"])], adapter(valid)).model_role(), "model");
    }
}
