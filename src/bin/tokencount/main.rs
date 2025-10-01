mod core;
mod encoding_utils;
mod file_tokens;

use crate::core::{
    count_str, get_encoding_for_model, get_model_for_encoding_name, get_valid_encodings,
    get_valid_models, map_tokens, tokenize_str as rs_tokenize_str, EncodingPick,
};
use crate::file_tokens::{count_dir, count_files, tokenize_dir, tokenize_files};
use anyhow::{Context, Result};
use clap::{ArgAction, Args, Parser, Subcommand, ValueHint};
use regex::Regex;
use serde_json::Value;
use std::{fs, path::PathBuf};
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// tokencount (Rust)
#[derive(Parser)]
#[command(
    name = "tokencount",
    author,
    version,
    about = "Tokenize strings/files/dirs and count tokens using tiktoken encodings."
)]
struct Cli {
    /// Global quiet flag (silence progress and minimize output)
    #[arg(short = 'q', long = "quiet", global = true, action = ArgAction::SetTrue)]
    quiet: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Tokenize a provided string.
    TokenizeStr(CommonArgsStr),
    /// Tokenize the contents of a file (comma or wildcard allowed).
    TokenizeFile(CommonArgsOnePath),
    /// Tokenize multiple files or a directory.
    TokenizeFiles(CommonArgsMultiPath),
    /// Tokenize all files in a directory.
    TokenizeDir(CommonArgsDir),
    /// Count tokens in a provided string.
    CountStr(CommonArgsStrCount),
    /// Count tokens in a file.
    CountFile(CommonArgsOnePath),
    /// Count tokens in multiple files or a directory.
    CountFiles(CommonArgsMultiPath),
    /// Count tokens in all files within a directory.
    CountDir(CommonArgsDir),
    /// Get model(s) for an encoding.
    GetModel(GetModelArgs),
    /// Get encoding for a model.
    GetEncoding(GetEncodingArgs),
    /// Map token integers to decoded strings.
    MapTokens(MapTokensArgs),
}

#[derive(Args)]
struct CommonBase {
    /// Model to use
    #[arg(
        short = 'm',
        long = "model",
        value_parser = clap::builder::PossibleValuesParser::new(get_valid_models()),
        default_value = "gpt-4o"
    )]
    model: String,

    /// Encoding to use directly
    #[arg(
        short = 'e',
        long = "encoding",
        value_parser = clap::builder::PossibleValuesParser::new(get_valid_encodings())
    )]
    encoding: Option<String>,

    /// Output JSON file
    #[arg(short = 'o', long = "output", value_hint = ValueHint::FilePath)]
    output: Option<PathBuf>,

    /// Include binary files
    #[arg(short = 'b', long = "include-binary", action = ArgAction::SetTrue)]
    include_binary: bool,

    /// Include hidden files and directories
    #[arg(short = 'H', long = "include-hidden", action = ArgAction::SetTrue)]
    include_hidden: bool,

    /// Output mapped tokens (decoded->id) instead of raw ints
    #[arg(short = 'M', long = "mapTokens", action = ArgAction::SetTrue)]
    map_tokens: bool,
}

#[derive(Args)]
struct CommonArgsStr {
    #[command(flatten)]
    base: CommonBase,
    /// The string to tokenize.
    string: String,
}

#[derive(Args)]
struct CommonArgsStrCount {
    #[command(flatten)]
    base: CommonBase,
    /// The string to count.
    string: String,
}

#[derive(Args)]
struct CommonArgsOnePath {
    #[command(flatten)]
    base: CommonBase,
    /// Path to file. Commas and wildcards supported.
    #[arg(value_hint = ValueHint::AnyPath)]
    file: String,
}

#[derive(Args)]
struct CommonArgsDir {
    #[command(flatten)]
    base: CommonBase,
    /// Directory path.
    #[arg(value_hint = ValueHint::DirPath)]
    directory: PathBuf,
    /// Do not recurse into subdirectories.
    #[arg(short = 'n', long = "no-recursive", action = ArgAction::SetTrue)]
    no_recursive: bool,
}

#[derive(Args)]
struct CommonArgsMultiPath {
    #[command(flatten)]
    base: CommonBase,
    /// Files or directories. Spaces or commas allowed. Wildcards supported.
    #[arg(value_hint = ValueHint::AnyPath)]
    input: Vec<String>,
    /// Do not recurse into subdirectories when a directory is given.
    #[arg(short = 'n', long = "no-recursive", action = ArgAction::SetTrue)]
    no_recursive: bool,
}

#[derive(Args)]
struct GetModelArgs {
    /// Encoding name
    #[arg(value_parser = clap::builder::PossibleValuesParser::new(get_valid_encodings()))]
    encoding: String,
}

#[derive(Args)]
struct GetEncodingArgs {
    /// Model name
    #[arg(value_parser = clap::builder::PossibleValuesParser::new(get_valid_models()))]
    model: String,
}

#[derive(Args)]
struct MapTokensArgs {
    #[command(flatten)]
    base: CommonBase,
    /// Tokens list. Spaces or commas allowed. Example: 123 456,789 10
    tokens: Vec<String>,
}

fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    let res = match cli.command {
        Commands::TokenizeStr(a) => {
            let pick = EncodingPick::new(Some(&a.base.model), a.base.encoding.as_deref())?;
            let v = rs_tokenize_str(&a.string, &pick, a.base.map_tokens, cli.quiet)?;
            output_or_print(v, a.base.output.as_ref())?;
            Ok(())
        }
        Commands::CountStr(a) => {
            let pick = EncodingPick::new(Some(&a.base.model), a.base.encoding.as_deref())?;
            let n = count_str(&a.string, &pick, cli.quiet)?;
            println!("{n}");
            Ok(())
        }
        Commands::TokenizeFile(a) => {
            let paths = parse_files(&[a.file])?;
            let pick = EncodingPick::new(Some(&a.base.model), a.base.encoding.as_deref())?;
            let v = tokenize_files(
                &paths,
                &pick,
                true, // treat list explicitly
                !a.base.include_binary,
                a.base.include_hidden,
                a.base.map_tokens,
                !cli.quiet,
            )?;
            output_or_print(v, a.base.output.as_ref())
        }
        Commands::TokenizeFiles(a) => {
            let paths = parse_files(&a.input)?;
            let pick = EncodingPick::new(Some(&a.base.model), a.base.encoding.as_deref())?;
            let v = tokenize_files(
                &paths,
                &pick,
                !a.no_recursive,
                !a.base.include_binary,
                a.base.include_hidden,
                a.base.map_tokens,
                !cli.quiet,
            )?;
            output_or_print(v, a.base.output.as_ref())
        }
        Commands::TokenizeDir(a) => {
            let pick = EncodingPick::new(Some(&a.base.model), a.base.encoding.as_deref())?;
            let v = tokenize_dir(
                &a.directory,
                &pick,
                !a.no_recursive,
                !a.base.include_binary,
                a.base.include_hidden,
                a.base.map_tokens,
                !cli.quiet,
            )?;
            output_or_print(v, a.base.output.as_ref())
        }
        Commands::CountFile(a) => {
            let paths = parse_files(&[a.file])?;
            let pick = EncodingPick::new(Some(&a.base.model), a.base.encoding.as_deref())?;
            let v = count_files(
                &paths,
                &pick,
                true,
                !a.base.include_binary,
                a.base.include_hidden,
                a.base.map_tokens,
                !cli.quiet,
            )?;
            output_or_print(v, a.base.output.as_ref())
        }
        Commands::CountFiles(a) => {
            let paths = parse_files(&a.input)?;
            let pick = EncodingPick::new(Some(&a.base.model), a.base.encoding.as_deref())?;
            let v = count_files(
                &paths,
                &pick,
                !a.no_recursive,
                !a.base.include_binary,
                a.base.include_hidden,
                a.base.map_tokens,
                !cli.quiet,
            )?;
            output_or_print(v, a.base.output.as_ref())
        }
        Commands::CountDir(a) => {
            let pick = EncodingPick::new(Some(&a.base.model), a.base.encoding.as_deref())?;
            let v = count_dir(
                &a.directory,
                &pick,
                !a.no_recursive,
                !a.base.include_binary,
                a.base.include_hidden,
                a.base.map_tokens,
                !cli.quiet,
            )?;
            output_or_print(v, a.base.output.as_ref())
        }
        Commands::GetModel(a) => {
            let m = get_model_for_encoding_name(&a.encoding)?;
            println!("{}", serde_json::to_string_pretty(&m)?);
            Ok(())
        }
        Commands::GetEncoding(a) => {
            let enc = get_encoding_for_model(&a.model)?;
            println!("{enc}");
            Ok(())
        }
        Commands::MapTokens(a) => {
            let pick = EncodingPick::new(Some(&a.base.model), a.base.encoding.as_deref())?;
            let toks = parse_tokens(&a.tokens)?;
            let v = map_tokens(&toks, &pick)?;
            output_or_print(v, a.base.output.as_ref())
        }
    };

    if let Err(e) = res {
        error!("{e:?}");
        std::process::exit(1);
    }
    Ok(())
}

fn init_tracing() {
    let fmt_layer = fmt::layer().with_target(false).with_ansi(true);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();
}

fn output_or_print(v: Value, out: Option<&PathBuf>) -> Result<()> {
    if let Some(p) = out {
        fs::write(p, serde_json::to_string_pretty(&v)?)?;
        info!("Output saved to {}", p.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&v)?);
    }
    Ok(())
}

/// Expand commas and wildcards into concrete paths. Errors if nothing matches.
fn parse_files(args: &[String]) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    let has_glob = |s: &str| s.contains('*') || s.contains('?') || s.contains('[');
    for raw in args {
        for part in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            if has_glob(part) {
                let mut matched = false;
                for entry in glob::glob(part).with_context(|| format!("bad glob: {part}"))? {
                    matched = true;
                    out.push(entry?);
                }
                if !matched {
                    anyhow::bail!("No files match pattern '{part}'.");
                }
            } else {
                let pb = PathBuf::from(part);
                if !pb.exists() {
                    anyhow::bail!("File or directory '{part}' does not exist.");
                }
                out.push(pb);
            }
        }
    }
    Ok(out)
}

fn parse_tokens(args: &[String]) -> Result<Vec<u32>> {
    let re = Regex::new(r"[,\s]+").unwrap();
    let mut out = Vec::new();
    for raw in args {
        for p in re.split(raw).filter(|s| !s.is_empty()) {
            let n: u32 = p.parse().with_context(|| format!("Invalid token '{p}'"))?;
            out.push(n);
        }
    }
    Ok(out)
}
