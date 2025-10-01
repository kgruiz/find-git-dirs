# find-git-dirs

`find-git-dirs` is a terminal user interface (TUI) tool that discovers every `.git` directory beneath one or more roots. It streams live progress, highlights fresh discoveries, and can export the final results as JSON. The project is licensed under the GNU General Public License v3.0 (GPL-3.0-only).

## Features

- Parallel filesystem scanning with per-root progress counters
- Live TUI powered by [ratatui](https://github.com/ratatui-org/ratatui)
- Configurable root paths, including following or skipping symlinks
- Recent discoveries panel to quickly inspect the latest repositories found
- Optional JSON output for post-processing

## Installation

You need a recent stable Rust toolchain with `cargo` installed. Install via `cargo install --path .` from a clone of this repository or build locally with `cargo build --release`.

```sh
cargo install --path .
```

## Usage

Run `find-git-dirs` without arguments to scan the current platform's default roots (the filesystem root on Unix-like systems, all drive letters on Windows):

```sh
cargo run --release
```

Key flags:

- `--root <PATH>`: add an explicit path to scan (repeatable)
- `<PATH>...`: provide positional paths to scan in addition to or instead of defaults
- `--no-follow-links`: disable following symlinks (on by default)
- `--json`: print the final list of `.git` directories as JSON instead of leaving the TUI active

While the TUI is running:

- Press `q`, `Esc`, or `Ctrl+C` to exit immediately.
- The header shows the overall scan rate, counters, and elapsed time.
- The per-root table shows scanning status and counts for each input root.
- The bottom panel lists the most recently discovered `.git` directories.

When run with `--json`, the program prints a JSON array of canonicalized `.git` directory paths after scanning completes, making it easy to feed into other tooling.

## Development

Clone the repo and use the standard Cargo workflow:

```sh
cargo fmt
cargo test
cargo run -- --help
```

The repository ignores the `target/` build directory. Before sending patches, please ensure formatting passes (`cargo fmt --check`) and tests succeed (`cargo test`).

## License

This project is released under the terms of the [GNU General Public License v3.0](LICENSE).
