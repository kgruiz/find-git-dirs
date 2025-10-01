use crate::core::{count_str, tokenize_str, EncodingPick};
use crate::encoding_utils::read_text_file;
use anyhow::{bail, Result};
use indexmap::IndexMap;
use lazy_static::lazy_static;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

lazy_static! {
    static ref BINARY_EXTS: BTreeSet<&'static str> = [
        ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".pbm", ".webp", ".avif", ".tiff", ".tif", ".ico",
        ".svgz", ".mp4", ".mkv", ".mov", ".avi", ".wmv", ".flv", ".webm", ".m4v", ".mpeg", ".mpg",
        ".3gp", ".3g2", ".mp3", ".wav", ".flac", ".ogg", ".aac", ".m4a", ".wma", ".aiff", ".ape",
        ".opus", ".zip", ".rar", ".7z", ".tar", ".gz", ".bz2", ".xz", ".lz", ".zst", ".cab",
        ".deb", ".rpm", ".pkg", ".iso", ".dmg", ".img", ".vhd", ".vmdk", ".exe", ".msi", ".bat",
        ".dll", ".so", ".bin", ".o", ".a", ".dylib", ".ttf", ".otf", ".woff", ".woff2", ".eot",
        ".pdf", ".ps", ".eps", ".psd", ".ai", ".indd", ".sketch", ".blend", ".stl", ".step",
        ".iges", ".fbx", ".glb", ".gltf", ".3ds", ".obj", ".cad", ".qcow2", ".vdi", ".vhdx",
        ".rom", ".bin", ".img", ".dat", ".pak", ".sav", ".nes", ".gba", ".nds", ".iso", ".jar",
        ".class", ".wasm",
    ]
    .into_iter()
    .collect();
}

fn is_hidden(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.starts_with('.'))
        .unwrap_or(false)
}

fn should_skip(p: &Path, include_hidden: bool, exclude_binary: bool) -> bool {
    if !include_hidden && is_hidden(p) {
        return true;
    }
    if exclude_binary {
        if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            if BINARY_EXTS.contains(&format!(".{}", ext.to_lowercase()).as_str()) {
                return true;
            }
        }
    }
    false
}

fn file_tokenize_value(path: &Path, pick: &EncodingPick, map_tokens: bool) -> Result<Value> {
    let text = read_text_file(path)?;
    let toks = tokenize_str(&text, pick, map_tokens, true)?;
    let count = if map_tokens {
        toks.as_object().map(|o| o.len()).unwrap_or(0)
    } else {
        toks.as_array().map(|a| a.len()).unwrap_or(0)
    };
    Ok(
        json!({ path.file_name().unwrap().to_string_lossy(): { "numTokens": count, "tokens": toks } }),
    )
}

pub fn tokenize_file(path: &Path, pick: &EncodingPick, map_tokens: bool) -> Result<Value> {
    file_tokenize_value(path, pick, map_tokens)
}

pub fn count_file(path: &Path, pick: &EncodingPick) -> Result<usize> {
    let text = read_text_file(path)?;
    count_str(&text, pick, true)
}

pub fn tokenize_dir(
    dir: &Path,
    pick: &EncodingPick,
    recursive: bool,
    exclude_binary: bool,
    include_hidden: bool,
    map_tokens: bool,
    show_progress: bool,
) -> Result<Value> {
    if !dir.is_dir() {
        bail!(
            "Given directory path '{}' is not a directory.",
            dir.display()
        );
    }
    if !include_hidden && is_hidden(dir) {
        return Ok(json!({}));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();

    if recursive {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                if !include_hidden && is_hidden(&p) {
                    continue;
                }
                subdirs.push(p);
            } else if p.is_file() {
                if should_skip(&p, include_hidden, exclude_binary) {
                    continue;
                }
                files.push(p);
            }
        }
    } else {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_file() {
                if should_skip(&p, include_hidden, exclude_binary) {
                    continue;
                }
                files.push(p);
            }
        }
    }

    let pb = if show_progress {
        let pb = indicatif::ProgressBar::new(files.len() as u64);
        pb.set_message("Tokenizing Directory");
        Some(pb)
    } else {
        None
    };

    let mut out: IndexMap<String, Value> = IndexMap::new();

    for f in files {
        let rel = f.file_name().unwrap().to_string_lossy().to_string();
        if let Some(pb) = &pb {
            pb.set_message(format!("Tokenizing {rel}"));
        }
        let val = file_tokenize_value(&f, pick, map_tokens)?;
        let (k, v) = val.as_object().unwrap().iter().next().unwrap();
        out.insert(k.clone(), v.clone());
        if let Some(pb) = &pb {
            pb.inc(1);
        }
    }

    if recursive {
        for sd in subdirs {
            let sub = tokenize_dir(
                &sd,
                pick,
                recursive,
                exclude_binary,
                include_hidden,
                map_tokens,
                show_progress,
            )?;
            let total = compute_total_tokens(&sub);
            out.insert(
                sd.file_name().unwrap().to_string_lossy().to_string(),
                json!({ "numTokens": total, "tokens": sub }),
            );
        }
    }

    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    Ok(serde_json::to_value(out)?)
}

fn compute_total_tokens(v: &Value) -> usize {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or(0) as usize,
        Value::Array(a) => a.len(),
        Value::Object(o) => {
            if let Some(Value::Number(n)) = o.get("numTokens") {
                return n.as_u64().unwrap_or(0) as usize;
            }
            o.values().map(compute_total_tokens).sum()
        }
        _ => 0,
    }
}

pub fn count_dir(
    dir: &Path,
    pick: &EncodingPick,
    recursive: bool,
    exclude_binary: bool,
    include_hidden: bool,
    map_tokens: bool,
    show_progress: bool,
) -> Result<Value> {
    if !dir.is_dir() {
        bail!("Given path '{}' is not a directory.", dir.display());
    }
    if !include_hidden && is_hidden(dir) {
        return Ok(json!({"numTokens": 0, "tokens": IndexMap::<String, Value>::new()}));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            if recursive {
                if !include_hidden && is_hidden(&p) {
                    continue;
                }
                subdirs.push(p);
            }
        } else if p.is_file() {
            if should_skip(&p, include_hidden, exclude_binary) {
                continue;
            }
            files.push(p);
        }
    }

    let pb = if show_progress {
        let pb = indicatif::ProgressBar::new(files.len() as u64);
        pb.set_message("Counting Tokens in Directory");
        Some(pb)
    } else {
        None
    };

    let mut mapping: IndexMap<String, Value> = IndexMap::new();
    let mut total = 0usize;

    for f in files {
        if let Some(pb) = &pb {
            pb.set_message(format!(
                "Counting Tokens in {}",
                f.file_name().unwrap().to_string_lossy()
            ));
        }
        let n = count_file(&f, pick)?;
        total += n;
        if map_tokens {
            mapping.insert(
                f.file_name().unwrap().to_string_lossy().to_string(),
                json!(n),
            );
        }
        if let Some(pb) = &pb {
            pb.inc(1);
        }
    }

    for sd in subdirs {
        let sub = count_dir(
            &sd,
            pick,
            recursive,
            exclude_binary,
            include_hidden,
            map_tokens,
            show_progress,
        )?;
        let sub_total = sub.get("numTokens").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
        total += sub_total;
        if map_tokens {
            mapping.insert(
                sd.file_name().unwrap().to_string_lossy().to_string(),
                sub.clone(),
            );
        }
    }

    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    if map_tokens {
        Ok(json!({ "numTokens": total, "tokens": mapping }))
    } else {
        Ok(json!({ "numTokens": total }))
    }
}

pub fn tokenize_files(
    inputs: &[PathBuf],
    pick: &EncodingPick,
    recursive: bool,
    exclude_binary: bool,
    include_hidden: bool,
    map_tokens: bool,
    show_progress: bool,
) -> Result<Value> {
    if inputs.len() == 1 {
        let p = &inputs[0];
        if p.is_file() {
            return tokenize_file(p, pick, map_tokens);
        } else if p.is_dir() {
            return tokenize_dir(
                p,
                pick,
                recursive,
                exclude_binary,
                include_hidden,
                map_tokens,
                show_progress,
            );
        } else {
            bail!("'{}' is neither a file nor a directory.", p.display());
        }
    }

    let pb = if show_progress {
        let pb = indicatif::ProgressBar::new(inputs.len() as u64);
        pb.set_message("Tokenizing File/Directory List");
        Some(pb)
    } else {
        None
    };

    let mut out: IndexMap<String, Value> = IndexMap::new();

    for path in inputs {
        if !include_hidden && is_hidden(path) {
            if let Some(pb) = &pb {
                pb.inc(1);
            }
            continue;
        }
        let key = path.file_name().unwrap().to_string_lossy().to_string();

        let v = if path.is_file() {
            if should_skip(path, include_hidden, exclude_binary) {
                if let Some(pb) = &pb {
                    pb.inc(1);
                }
                continue;
            }
            tokenize_file(path, pick, map_tokens)?
        } else if path.is_dir() {
            let sub = tokenize_dir(
                path,
                pick,
                recursive,
                exclude_binary,
                include_hidden,
                map_tokens,
                show_progress,
            )?;
            let total = compute_total_tokens(&sub);
            json!({ "numTokens": total, "tokens": sub })
        } else {
            bail!(
                "Entry '{}' is neither a file nor a directory.",
                path.display()
            );
        };

        if let Some(obj) = v.as_object() {
            if obj.len() == 1 && obj.contains_key(&key) {
                out.insert(key.clone(), obj.get(&key).unwrap().clone());
            } else {
                out.insert(key, v);
            }
        } else {
            out.insert(key, v);
        }

        if let Some(pb) = &pb {
            pb.inc(1);
        }
    }

    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    Ok(serde_json::to_value(out)?)
}

pub fn count_files(
    inputs: &[PathBuf],
    pick: &EncodingPick,
    recursive: bool,
    exclude_binary: bool,
    include_hidden: bool,
    map_tokens: bool,
    show_progress: bool,
) -> Result<Value> {
    if inputs.len() == 1 {
        let p = &inputs[0];
        if p.is_file() {
            let n = count_file(p, pick)?;
            if map_tokens {
                return Ok(json!({ p.file_name().unwrap().to_string_lossy().to_string(): n }));
            }
            return Ok(json!(n));
        } else if p.is_dir() {
            return count_dir(
                p,
                pick,
                recursive,
                exclude_binary,
                include_hidden,
                map_tokens,
                show_progress,
            );
        } else {
            bail!("'{}' is neither a file nor a directory.", p.display());
        }
    }

    let pb = if show_progress {
        let pb = indicatif::ProgressBar::new(inputs.len() as u64);
        pb.set_message("Counting Tokens in File/Directory List");
        Some(pb)
    } else {
        None
    };

    let mut out: IndexMap<String, Value> = IndexMap::new();
    let mut total = 0usize;

    for path in inputs {
        if !include_hidden && is_hidden(path) {
            if let Some(pb) = &pb {
                pb.inc(1);
            }
            continue;
        }
        let key = path.file_name().unwrap().to_string_lossy().to_string();

        if path.is_file() {
            if should_skip(path, include_hidden, exclude_binary) {
                if let Some(pb) = &pb {
                    pb.inc(1);
                }
                continue;
            }
            let n = count_file(path, pick)?;
            total += n;
            if map_tokens {
                out.insert(key, json!(n));
            }
        } else if path.is_dir() {
            let sub = count_dir(
                path,
                pick,
                recursive,
                exclude_binary,
                include_hidden,
                map_tokens,
                show_progress,
            )?;
            let sub_total = sub.get("numTokens").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
            total += sub_total;
            if map_tokens {
                out.insert(key, sub);
            }
        } else {
            bail!(
                "Entry '{}' is neither a file nor a directory.",
                path.display()
            );
        }

        if let Some(pb) = &pb {
            pb.inc(1);
        }
    }

    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    if map_tokens {
        Ok(json!({ "numTokens": total, "tokens": out }))
    } else {
        Ok(json!(total))
    }
}
