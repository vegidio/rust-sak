//! Where each dependency lives under the caller's root.

use std::path::{Path, PathBuf};

use super::Source;

/// The folder `source` installs into under `root`: `<root>/<name>/<version>`, the last two segments of its Hugging
/// Face folder, so `deps/cuda/13.3.0` installs into `<root>/cuda/13.3.0`. The root is always the caller's.
pub fn dir_for(root: &Path, source: &Source) -> PathBuf {
    root.join(source.name()).join(source.version())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_maps_to_its_name_and_version_under_the_root() {
        let source = Source::parse("https://huggingface.co/vegidio/general/tree/main/deps/cuda/13.3.0").unwrap();
        assert_eq!(
            dir_for(Path::new("/data/io.vinicius.ai"), &source),
            Path::new("/data/io.vinicius.ai/cuda/13.3.0")
        );
        assert_eq!(
            dir_for(Path::new("/elsewhere"), &source),
            Path::new("/elsewhere/cuda/13.3.0")
        );
    }
}
