use anyhow::{bail, Context, Result};
use chardetng::EncodingDetector;
use encoding_rs::{mem, UTF_8, WINDOWS_1252};
use std::{fs, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct UnsupportedEncodingError {
    pub message: String,
}

pub fn read_text_file(path: &Path) -> Result<String> {
    if !path.exists() {
        bail!("File not found: {}", path.display());
    }
    let data = fs::read(path).with_context(|| format!("read failed: {}", path.display()))?;
    if data.is_empty() {
        return Ok(String::new());
    }

    let mut det = EncodingDetector::new();
    det.feed(&data, true);
    let enc = det.guess(None, true);

    for e in [enc, WINDOWS_1252, UTF_8] {
        let (cow, _, had_errors) = e.decode(&data);
        if !had_errors {
            return Ok(cow.into_owned());
        }
    }

    let fallback = mem::decode_latin1(&data).into_owned();
    if !fallback.is_empty() {
        return Ok(fallback);
    }

    Err(UnsupportedEncodingError {
        message: format!(
            "Failed to decode using encodings: {}, windows-1252, utf-8, latin-1\nFile: {}",
            enc.name(),
            path.display()
        ),
    }
    .into())
}
