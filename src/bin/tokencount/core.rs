use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use lazy_static::lazy_static;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tiktoken_rs::{
    cl100k_base, get_bpe_from_model, o200k_base, p50k_base, p50k_edit, r50k_base, CoreBPE,
};

lazy_static! {
    static ref MODEL_MAPPINGS: BTreeMap<&'static str, &'static str> = BTreeMap::from([
        ("gpt-4o", "o200k_base"),
        ("gpt-4", "cl100k_base"),
        ("gpt-3.5-turbo", "cl100k_base"),
        ("gpt-3.5", "cl100k_base"),
        ("gpt-35-turbo", "cl100k_base"),
        ("davinci-002", "cl100k_base"),
        ("babbage-002", "cl100k_base"),
        ("text-embedding-ada-002", "cl100k_base"),
        ("text-embedding-3-small", "cl100k_base"),
        ("text-embedding-3-large", "cl100k_base"),
        ("text-davinci-003", "p50k_base"),
        ("text-davinci-002", "p50k_base"),
        ("text-davinci-001", "r50k_base"),
        ("text-curie-001", "r50k_base"),
        ("text-babbage-001", "r50k_base"),
        ("text-ada-001", "r50k_base"),
        ("davinci", "r50k_base"),
        ("curie", "r50k_base"),
        ("babbage", "r50k_base"),
        ("ada", "r50k_base"),
        ("code-davinci-002", "p50k_base"),
        ("code-davinci-001", "p50k_base"),
        ("code-cushman-002", "p50k_base"),
        ("code-cushman-001", "p50k_base"),
        ("davinci-codex", "p50k_base"),
        ("cushman-codex", "p50k_base"),
        ("text-davinci-edit-001", "p50k_edit"),
        ("code-davinci-edit-001", "p50k_edit"),
        ("text-similarity-davinci-001", "r50k_base"),
        ("text-similarity-curie-001", "r50k_base"),
        ("text-similarity-babbage-001", "r50k_base"),
        ("text-similarity-ada-001", "r50k_base"),
        ("text-search-davinci-doc-001", "r50k_base"),
        ("text-search-curie-doc-001", "r50k_base"),
        ("text-search-babbage-doc-001", "r50k_base"),
        ("text-search-ada-doc-001", "r50k_base"),
        ("code-search-babbage-code-001", "r50k_base"),
        ("code-search-ada-code-001", "r50k_base"),
        ("gpt2", "gpt2"),
        ("gpt-2", "gpt2"),
    ]);
    static ref VALID_MODELS: Vec<&'static str> = MODEL_MAPPINGS.keys().copied().collect();
    static ref VALID_ENCODINGS: Vec<&'static str> = {
        let mut s = BTreeSet::new();
        for &enc in MODEL_MAPPINGS.values() {
            s.insert(enc);
        }
        s.into_iter().collect()
    };
}

pub fn get_valid_models() -> Vec<&'static str> {
    VALID_MODELS.clone()
}

pub fn get_valid_encodings() -> Vec<&'static str> {
    VALID_ENCODINGS.clone()
}

pub fn get_model_for_encoding_name(encoding: &str) -> Result<Value> {
    if !VALID_ENCODINGS.contains(&encoding) {
        bail!("Invalid encoding name: {encoding}");
    }
    let mut matches: Vec<&str> = MODEL_MAPPINGS
        .iter()
        .filter_map(|(m, e)| if *e == encoding { Some(*m) } else { None })
        .collect();
    matches.sort_unstable();
    Ok(json!(matches))
}

pub fn get_encoding_for_model(model: &str) -> Result<String> {
    MODEL_MAPPINGS
        .get(model)
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!(format!("Invalid model: {model}")))
}

#[allow(dead_code)]
pub struct EncodingPick {
    pub model: Option<String>,
    pub encoding: String,
    pub bpe: CoreBPE,
}

impl EncodingPick {
    pub fn new(model: Option<&str>, encoding_name: Option<&str>) -> Result<Self> {
        let chosen_encoding = match (model, encoding_name) {
            (Some(m), Some(e)) => {
                let mapped = get_encoding_for_model(m)?;
                if mapped != e {
                    bail!(format!(
                        "Model {m} does not have encoding name {e}. Valid for {m}: {mapped}"
                    ));
                }
                e.to_string()
            }
            (Some(m), None) => get_encoding_for_model(m)?,
            (None, Some(e)) => {
                if !VALID_ENCODINGS.contains(&e) {
                    bail!("Invalid encoding name: {e}");
                }
                e.to_string()
            }
            (None, None) => "gpt-4o".to_string(),
        };

        let bpe = build_bpe(model, &chosen_encoding)?;
        Ok(Self {
            model: model.map(|s| s.to_string()),
            encoding: chosen_encoding,
            bpe,
        })
    }
}

fn build_bpe(model: Option<&str>, encoding: &str) -> Result<CoreBPE> {
    if let Some(m) = model {
        if let Ok(bpe) = get_bpe_from_model(m) {
            return Ok(bpe);
        }
    }
    let bpe = match encoding {
        "o200k_base" => o200k_base().context("o200k_base not available")?,
        "cl100k_base" => cl100k_base().context("cl100k_base not available")?,
        "p50k_base" => p50k_base().context("p50k_base not available")?,
        "p50k_edit" => p50k_edit().context("p50k_edit not available")?,
        "r50k_base" | "gpt2" => r50k_base().context("r50k_base not available")?,
        other => bail!("Unsupported encoding: {other}"),
    };
    Ok(bpe)
}

pub fn tokenize_str(
    text: &str,
    pick: &EncodingPick,
    map_tokens: bool,
    _quiet: bool,
) -> Result<Value> {
    let toks = pick.bpe.encode_ordinary(text);
    if map_tokens {
        let mut mapped: IndexMap<String, u32> = IndexMap::new();
        for &t in &toks {
            let s = pick
                .bpe
                .decode(vec![t])
                .with_context(|| format!("decode failed for token {t}"))?;
            mapped.insert(s, t);
        }
        Ok(serde_json::to_value(mapped)?)
    } else {
        Ok(serde_json::to_value(toks)?)
    }
}

pub fn count_str(text: &str, pick: &EncodingPick, _quiet: bool) -> Result<usize> {
    let toks = pick.bpe.encode_ordinary(text);
    Ok(toks.len())
}

pub fn map_tokens(tokens: &[u32], pick: &EncodingPick) -> Result<Value> {
    let mut mapped: IndexMap<String, u32> = IndexMap::new();
    for &t in tokens {
        let s = pick
            .bpe
            .decode(vec![t])
            .with_context(|| format!("decode failed for token {t}"))?;
        mapped.insert(s, t);
    }
    Ok(serde_json::to_value(mapped)?)
}
