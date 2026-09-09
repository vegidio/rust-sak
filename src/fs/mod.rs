//! Filesystem helpers and hardened archive extraction.
//!
//! Two groups of things live here. The first is the small filesystem chores an application repeats everywhere:
//! [`file_exists`], temporary files and directories ([`mk_temp_file`], [`mk_temp_dir`]), per-application
//! configuration paths ([`user_config_dir`], [`mk_user_config_file`]), directory listing ([`list_path`]) and
//! filtered copying ([`copy_files`], [`move_files`]). All of it is **synchronous** — plain [`std::fs`], no async
//! runtime.
//!
//! The second is an archive extractor for ZIP, 7z and TAR.XZ ([`extract`], or [`unzip`] / [`un7zip`] / [`untar_xz`]
//! directly) that treats every entry as hostile. Entry names are validated identically on every platform, and each
//! path is then resolved *through* an anchored directory handle, so a symbolic link planted by an earlier entry —
//! or one already sitting in the target directory — cannot be traversed even though the path walking through it
//! looks ordinary. See `README.md` in this module for what the guarantees actually are.
//!
//! Temporary handles are RAII: dropping a [`TempDir`] or [`NamedTempFile`] removes it, including on an early `?` or
//! a panic, so nothing leaks when a function returns down an unexpected path.
//!
//! ```
//! use rust_sak::fs::{copy_files, file_exists, list_path, mk_temp_dir, CopyOptions, ListOptions};
//!
//! let source = mk_temp_dir("source")?;
//! std::fs::write(source.path().join("notes.txt"), b"hello")?;
//! std::fs::write(source.path().join("debug.log"), b"noise")?;
//! assert!(file_exists(source.path().join("notes.txt")));
//!
//! // List what matches, then copy exactly that.
//! let wanted = list_path(source.path(), &ListOptions::new().extension("txt"))?;
//! assert_eq!(wanted.len(), 1);
//!
//! let dest = mk_temp_dir("dest")?;
//! let summary = copy_files(wanted, dest.path(), &CopyOptions::new())?;
//! assert_eq!(summary.files, 1);
//! assert_eq!(summary.bytes, 5);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod copy_files;
mod copy_move;
mod copy_options;
mod error;
mod ext_filter;
mod extract;
mod extract_archive;
mod extract_options;
mod extract_root;
mod file_exists;
mod list_options;
mod list_path;
mod mk_temp_dir;
mod mk_temp_file;
mod mk_user_config_dir;
mod mk_user_config_file;
mod move_files;
mod safe_path;
mod un7zip;
mod untar_xz;
mod unzip;
mod user_config_dir;

pub use copy_files::copy_files;
pub use copy_options::{CopyOptions, CopySummary};
pub use error::{FsError, Limit, Result};
pub use extract_archive::extract;
pub use extract_options::{ExtractOptions, ExtractSummary};
pub use file_exists::file_exists;
pub use list_options::ListOptions;
pub use list_path::list_path;
pub use mk_temp_dir::{mk_temp_dir, mk_temp_dir_in};
pub use mk_temp_file::{mk_temp_file, mk_temp_file_in};
pub use mk_user_config_dir::mk_user_config_dir;
pub use mk_user_config_file::mk_user_config_file;
pub use move_files::move_files;
pub use tempfile::{NamedTempFile, TempDir};
pub use un7zip::un7zip;
pub use untar_xz::untar_xz;
pub use unzip::unzip;
pub use user_config_dir::user_config_dir;

#[cfg(test)]
mod tests;
