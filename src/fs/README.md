# `fs` module

Filesystem helpers and a **hardened archive extractor** for ZIP, 7z and TAR.XZ. Everything here is **synchronous** (plain `std::fs`, no async runtime).

## Enabling

The module is gated behind the `fs` Cargo feature:

```toml
[dependencies]
rust-sak = { version = "2", features = ["fs"] }
```

> Building this feature compiles **xz from vendored C sources**, so a C compiler is required. No system `liblzma` is needed.

## Public API

### Existence and temporary files

| Function          | Signature                                                                                | What it does                                    |
|-------------------|------------------------------------------------------------------------------------------|-------------------------------------------------|
| `file_exists`     | `fn file_exists(path: impl AsRef<Path>) -> bool`                                         | True only for a regular file; follows symlinks. |
| `mk_temp_dir`     | `fn mk_temp_dir(prefix: &str) -> Result<TempDir>`                                        | Temporary directory under the system temp dir.  |
| `mk_temp_dir_in`  | `fn mk_temp_dir_in(directory: impl AsRef<Path>, prefix: &str) -> Result<TempDir>`        | Same, in a directory you choose.                |
| `mk_temp_file`    | `fn mk_temp_file(prefix: &str) -> Result<NamedTempFile>`                                 | Temporary file under the system temp dir.       |
| `mk_temp_file_in` | `fn mk_temp_file_in(directory: impl AsRef<Path>, prefix: &str) -> Result<NamedTempFile>` | Same, in a directory you choose.                |

Temporary handles are **RAII**: dropping a `TempDir` or `NamedTempFile` removes it, including on an early `?` or a panic. Call `.keep()` to opt out.

Both are this crate's own types, wrapping [`tempfile`](https://crates.io/crates/tempfile) rather than re-exporting it, so `tempfile`'s major version is not part of this crate's public API and you can depend on a different one yourself. They expose `path()`, `keep()` and `AsRef<Path>`; `NamedTempFile` also derefs to `std::fs::File`, so `Read`/`Write`/`Seek` work directly on it. Call `.into_inner()` when you need the wrapped `tempfile` value itself.

### User configuration

| Function              | Signature                                                                          | What it does                                          |
|-----------------------|------------------------------------------------------------------------------------|-------------------------------------------------------|
| `user_config_dir`     | `fn user_config_dir(name: &str, sub_path: impl AsRef<Path>) -> Result<PathBuf>`    | Computes the path. **No I/O.**                        |
| `mk_user_config_dir`  | `fn mk_user_config_dir(name: &str, sub_path: impl AsRef<Path>) -> Result<PathBuf>` | Creates it (mode `0o755`) and returns the path.       |
| `mk_user_config_file` | `fn mk_user_config_file(name: &str, file_path: impl AsRef<Path>) -> Result<File>`  | Creates parents, opens the file read/write (`0o644`). |

The path is `<platform config dir>/<name>/<sub_path>` — `~/.config/…` on Linux, `~/Library/Application Support/…` on macOS, `%APPDATA%\…` on Windows. `mk_user_config_file` **does not truncate**, so an existing config survives being opened.

Both `name` and `sub_path` are validated with the same rules as archive entries, so a `sub_path` of `../../.ssh/authorized_keys` is refused rather than quietly resolved.

### Listing, copying, moving

| Function     | Signature                                                                                                   |
|--------------|-------------------------------------------------------------------------------------------------------------|
| `list_path`  | `fn list_path(directory: impl AsRef<Path>, options: &ListOptions) -> Result<Vec<PathBuf>>`                  |
| `copy_files` | `fn copy_files<I, P>(sources: I, dest_dir: impl AsRef<Path>, options: &CopyOptions) -> Result<CopySummary>` |
| `move_files` | `fn move_files<I, P>(sources: I, dest_dir: impl AsRef<Path>, options: &CopyOptions) -> Result<CopySummary>` |

`sources` is any `IntoIterator` of paths, so an array of `&str`, a `Vec<PathBuf>`, or the output of `list_path` all work directly.

`ListOptions` defaults to **files only, one level, unfiltered**; `CopyOptions` to **non-recursive, flattened, unfiltered**. Both are consuming builders, and extension filters are case-insensitive with an optional leading dot, accumulating across calls:

```rust
use rust_sak::fs::{CopyOptions, ListOptions};

let listing = ListOptions::new().recursive(true).extensions(["jpg", "png"]);
let copying = CopyOptions::new().recursive(true).preserve_structure(true).extension(".JPG");
```

`move_files` copies and then deletes — so it works across filesystems, where a rename fails. It removes **only files it copied**, never a directory, since a directory may still hold entries the filter skipped.

### Extraction

| Function     | Signature                                                                                                                 |
|--------------|---------------------------------------------------------------------------------------------------------------------------|
| `extract`    | `fn extract(archive: impl AsRef<Path>, target_dir: impl AsRef<Path>, options: &ExtractOptions) -> Result<ExtractSummary>` |
| `extract_as` | `fn extract_as(format: ArchiveFormat, archive: ..., target_dir: ..., options: ...) -> Result<ExtractSummary>`             |
| `unzip`      | *(same three parameters as `extract`)* — ZIP                                                                              |
| `un7zip`     | *(same)* — 7z                                                                                                             |
| `untar_xz`   | *(same)* — TAR.XZ                                                                                                         |

`extract` dispatches on the file name: `.zip`, `.7z`, `.tar.xz`, `.txz` (case-insensitive). An unrecognised extension is an error, never a guess — nothing is sniffed from the contents.

When you already know the format from somewhere more reliable than a file name — a `Content-Type` header, or a body downloaded into an `mk_temp_file` with no meaningful name — name it with `ArchiveFormat` and call `extract_as`, which ignores the name entirely:

```rust
# use rust_sak::fs::{extract_as, ArchiveFormat, ExtractOptions};
# fn main() -> Result<(), rust_sak::fs::FsError> {
assert_eq!(ArchiveFormat::from_path("backup.tar.xz"), Some(ArchiveFormat::TarXz));

# if false {
extract_as(ArchiveFormat::Zip, "/tmp/download.bin", "/tmp/out", &ExtractOptions::new())?;
# }
# Ok(())
# }
```

```rust,no_run
use rust_sak::fs::{extract, ExtractOptions};

let summary = extract("upload.zip", "/tmp/out", &ExtractOptions::new()
    .max_total_bytes(100 << 20)
    .max_file_bytes(10 << 20)
    .max_entries(10_000)
    .symlinks(false))?;

println!("{} files, {} skipped", summary.files, summary.skipped);
# Ok::<(), rust_sak::fs::FsError>(())
```

## What "hardened" means

Extraction assumes every entry is hostile. Two independent layers have to agree before anything is written:

1. **Name validation**, applied identically on *every* platform — so an archive that is dangerous on Windows is refused on Linux too. Rejected: absolute paths, UNC paths, drive letters, any `..` component, reserved Windows device names (`NUL`, `COM1`, including the superscript `COM¹` forms behind CVE-2024-51756), and embedded NUL bytes.
2. **An anchored directory handle** ([`cap-std`](https://github.com/bytecodealliance/cap-std)), which resolves every path *through* the target directory at the OS level. This is what catches escapes no textual check can see — a symbolic link planted by an earlier entry, or one already sitting in the target directory, where the path that walks through it looks perfectly ordinary.

On top of that:

- **Symbolic links** are checked lexically *and* re-resolved through the handle after creation, then deleted if they escape. Dangling links are allowed; they are legal and common.
- **Modes** are masked to `0o777`, so setuid, setgid and the sticky bit can never survive extraction. `file_mode` overrides regular files only.
- **Directory modes are deferred** and applied deepest-first, because a directory the archive wants at `0o500` cannot be tightened while its children are still being written.
- **Entries a filesystem cannot safely reproduce** — device nodes, FIFOs, sockets, hard links — are skipped and counted, never created.
- **Limits are reserved before writing**, using the size the entry *declares*, so a header claiming to be tiny cannot open a handle and then stream gigabytes. An entry that outgrows its own header is a `DeclaredSizeMismatch`, distinct from a limit you set.

`ExtractSummary` counts `files`, `directories`, `symlinks`, `skipped` and `bytes` — the counters are the only way skipped behaviour becomes observable, since by definition it leaves nothing on disk.

## Errors

Everything returns `fs::Result<T>` (`Result<T, FsError>`). `FsError` wraps the underlying libraries (`Io`, which also covers `tar` and `liblzma`; `Zip`; `SevenZ`) and adds this module's own: `EmptyName`, `NoConfigDir`, `UnknownArchiveFormat` (carrying the file name it rejected), `IllegalPath`, `IllegalSymlink`, `DeclaredSizeMismatch` and `LimitExceeded` (carrying a typed `Limit`).

Rejections happen **before** anything is written, so an `IllegalPath` or `IllegalSymlink` means nothing landed outside the target directory. Extraction stops at the first bad entry; entries already written stay on disk.
