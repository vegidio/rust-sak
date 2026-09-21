# rust-sak

A "Swiss Army Knife" of reusable Rust building blocks: hashing, HTTP fetching, hardened archive extraction, image codecs, memoization, OTLP telemetry and hardware probes.

## ⬇️ Installation

This library can be installed using Cargo. To do that, run the following command in your project's root directory:

```bash
cargo add rust-sak --features crypto,fs
```

Or add it directly to your `Cargo.toml`:

```toml
[dependencies]
rust-sak = { version = "2", features = ["crypto", "fs"] }
```

The crate is a collection of independent modules, each gated behind its own Cargo feature so consumers compile only what they need.

> [!NOTE]
> **There are no default features** — enable exactly the ones you want, otherwise the crate compiles to nothing. See [Modules](#-modules) for the full list.

## 🧩 Modules

| Feature      | Module                             | What it does                                                                                                     | Async?          |
|--------------|------------------------------------|------------------------------------------------------------------------------------------------------------------|-----------------|
| `crypto`     | [`crypto`](src/crypto/README.md)   | SHA-256 and XXH3-64 hashing of bytes, strings and files; always lowercase hex                                    | no              |
| `fetch`      | [`fetch`](src/fetch/README.md)     | Reusable `reqwest` client with Fibonacci-backoff retries and resumable streaming downloads with live progress    | **yes** (Tokio) |
| `fs`         | [`fs`](src/fs/README.md)           | Filesystem helpers, RAII temp handles, and a hardened ZIP/7z/TAR.XZ extractor that treats every entry as hostile | no              |
| `image`      | [`image`](src/image/README.md)     | Encode and decode 8 image formats behind one uniform API — bmp, gif, jpeg, png, tiff, avif, heif, webp           | no              |
| `image-raw`  |                                    | Adds camera RAW/DNG decoding to `image` — dng, nef, cr2/cr3, arw, raf and ~20 more                               | no              |
| `memo`       | [`memo`](src/memo/README.md)       | Memoization with pluggable storage: memory, disk, or memory-over-disk, with concurrent calls coalesced           | no              |
| `memo-async` |                                    | Adds `Memo::get_or_compute_async` for computations that are themselves futures                                   | **yes** (Tokio) |
| `o11y`       | [`o11y`](src/o11y/README.md)       | Logs, metrics and traces to an OpenTelemetry collector over OTLP, exported on a background thread                | no              |
| `sysinfo`    | [`sysinfo`](src/sysinfo/README.md) | CPU, memory and GPU probes, asked through each platform's own interface — no subprocesses                        | no              |

Each module's README covers its API and guarantees in full; the same content is rendered in the [API documentation](https://docs.rs/rust-sak).

## 🔧 Build requirements

The minimum supported Rust version is **1.95**. Most features need nothing beyond a Rust compiler. Two do:

- **`fs`** compiles xz from vendored C sources on first build, so a **C compiler** is required.
- **`image`** downloads prebuilt static avif/heif/webp codec binaries on first build, so an **internet connection** is required — or point `AVIF_BINARIES_DIR`, `HEIF_BINARIES_DIR` and `WEBP_BINARIES_DIR` at pre-extracted archives for an offline build. They link statically, so no system libraries are needed at runtime.

## 🤖 Development

```bash
cargo test --all-features            # the full suite
cargo test --doc --all-features      # doctests, including the module READMEs
scripts/coverage.sh                  # HTML coverage report (all features)
scripts/coverage.sh --by-feature     # per-feature coverage, each compiled in isolation
cargo run --example playground --features fetch
```

CI additionally compiles every feature in isolation (`--no-default-features --features <f>`), which is how a feature that only builds because a sibling happened to pull in its dependency gets caught.

The repository is a cargo workspace: the `rust-sak` package at the root, plus the `o11y-macros` package in `macros/`, a proc-macro crate supplying `#[instrument]` for the `o11y` feature. A proc-macro crate cannot be a module of a normal one, which is the only reason the workspace exists. Neither member is published to crates.io, so `rust-sak` depends on `o11y-macros` by path with no version and both carry `publish = false`; a git dependency resolves the path because cargo clones the whole repository.

## 📝 License

**rust-sak** is released under the Apache 2.0 License. See [LICENSE](LICENSE) for details.

## 👨🏾‍💻 Author

Vinicius Egidio ([vinicius.io](http://vinicius.io))
