use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use super::Result;

/// A temporary directory that removes itself when dropped.
///
/// Returned by [`mk_temp_dir`](super::mk_temp_dir) and [`mk_temp_dir_in`](super::mk_temp_dir_in). Dropping it deletes
/// the directory and everything inside, including on an early `?` or a panic; [`TempDir::keep`] defuses that.
///
/// This is this crate's own type wrapping [`tempfile::TempDir`], deliberately rather than a re-export, so that
/// `tempfile`'s major version is not part of rust-sak's public API. [`TempDir::into_inner`] hands back the wrapped
/// value for code that needs to name `tempfile`'s type directly.
#[derive(Debug)]
pub struct TempDir(tempfile::TempDir);

impl TempDir {
    /// Wraps a `tempfile` handle.
    pub(super) fn new(inner: tempfile::TempDir) -> Self {
        TempDir(inner)
    }

    /// The path of the temporary directory.
    pub fn path(&self) -> &Path {
        self.0.path()
    }

    /// Keeps the directory on disk instead of deleting it, returning its path.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`](super::FsError::Io) if the directory could not be persisted.
    pub fn keep(self) -> Result<PathBuf> {
        Ok(self.0.keep())
    }

    /// Unwraps the [`tempfile::TempDir`] inside, for interoperating with code that names it directly.
    pub fn into_inner(self) -> tempfile::TempDir {
        self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

/// A named temporary file that deletes itself when dropped.
///
/// Returned by [`mk_temp_file`](super::mk_temp_file) and [`mk_temp_file_in`](super::mk_temp_file_in). Dropping it
/// deletes the file, including on an early `?` or a panic; [`NamedTempFile::keep`] defuses that.
///
/// As [`TempDir`], this is this crate's own type rather than a re-export of [`tempfile::NamedTempFile`], so that
/// `tempfile`'s major version stays out of rust-sak's public API. It [`Deref`]s to [`File`], so the usual
/// [`Read`]/[`Write`]/[`Seek`] calls work directly on it.
#[derive(Debug)]
pub struct NamedTempFile(tempfile::NamedTempFile);

impl NamedTempFile {
    /// Wraps a `tempfile` handle.
    pub(super) fn new(inner: tempfile::NamedTempFile) -> Self {
        NamedTempFile(inner)
    }

    /// The path of the temporary file.
    pub fn path(&self) -> &Path {
        self.0.path()
    }

    /// Keeps the file on disk instead of deleting it, returning the open handle and its path.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`](super::FsError::Io) if the file could not be persisted.
    pub fn keep(self) -> Result<(File, PathBuf)> {
        // `PersistError` carries the file back alongside the failure so the caller could retry; nothing here can, so
        // only the underlying I/O error is kept.
        self.0.keep().map_err(|err| err.error.into())
    }

    /// Reopens the file as a second, independent handle with its own cursor.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`](super::FsError::Io) if the file could not be reopened.
    pub fn reopen(&self) -> Result<File> {
        Ok(self.0.reopen()?)
    }

    /// Unwraps the [`tempfile::NamedTempFile`] inside, for interoperating with code that names it directly.
    pub fn into_inner(self) -> tempfile::NamedTempFile {
        self.0
    }
}

impl AsRef<Path> for NamedTempFile {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Deref for NamedTempFile {
    type Target = File;

    fn deref(&self) -> &File {
        self.0.as_file()
    }
}

impl DerefMut for NamedTempFile {
    fn deref_mut(&mut self) -> &mut File {
        self.0.as_file_mut()
    }
}

impl Read for NamedTempFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.as_file_mut().read(buf)
    }
}

impl Write for NamedTempFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.as_file_mut().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.as_file_mut().flush()
    }
}

impl Seek for NamedTempFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.as_file_mut().seek(pos)
    }
}
