use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::extract::{Budget, EntryKind, permissions};
use super::mk_user_config_dir::mk_config_dir_at;
use super::mk_user_config_file::mk_config_file_at;
use super::safe_path::{validate_entry_name, validate_symlink_target};
use super::un7zip::classify;
use super::user_config_dir::config_path;
use super::*;

/// Splits a validated entry name, panicking on rejection — for cases where acceptance is the precondition, not the
/// thing under test.
fn components(name: &str) -> Vec<String> {
    validate_entry_name(name).expect("name should be accepted")
}

// --- file_exists tests ---

#[test]
fn file_exists_reports_true_for_a_regular_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, b"hi").unwrap();

    assert!(file_exists(&path));
}

#[test]
fn file_exists_reports_false_for_a_missing_path() {
    let dir = TempDir::new().unwrap();

    assert!(!file_exists(dir.path().join("nope.txt")));
}

#[test]
fn file_exists_reports_false_for_a_directory() {
    let dir = TempDir::new().unwrap();

    assert!(!file_exists(dir.path()));
}

#[test]
fn file_exists_reports_false_for_an_empty_path() {
    assert!(!file_exists(""));
}

#[cfg(unix)]
#[test]
fn file_exists_follows_a_symlink_to_a_file() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("target.txt");
    let link = dir.path().join("link.txt");
    fs::write(&target, b"hi").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    assert!(file_exists(&link));
}

#[cfg(unix)]
#[test]
fn file_exists_reports_false_for_a_dangling_symlink() {
    let dir = TempDir::new().unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(dir.path().join("missing.txt"), &link).unwrap();

    assert!(!file_exists(&link));
}

// --- temp tests ---

#[test]
fn mk_temp_dir_creates_a_directory_with_the_prefix() {
    let dir = mk_temp_dir("rust_sak_fs_").unwrap();

    assert!(dir.path().is_dir());
    let name = dir.path().file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("rust_sak_fs_"), "unexpected name: {name}");
}

#[test]
fn mk_temp_dir_removes_the_directory_on_drop() {
    let dir = mk_temp_dir("rust_sak_fs_").unwrap();
    let path = dir.path().to_path_buf();
    drop(dir);

    assert!(!path.exists());
}

#[test]
fn mk_temp_dir_keeps_the_directory_when_kept() {
    let dir = mk_temp_dir("rust_sak_fs_").unwrap();
    let path = dir.keep();

    assert!(path.is_dir());
    fs::remove_dir_all(&path).unwrap();
}

#[test]
fn mk_temp_dir_in_places_the_directory_in_the_given_parent() {
    let parent = TempDir::new().unwrap();
    let dir = mk_temp_dir_in(parent.path(), "child_").unwrap();

    assert_eq!(dir.path().parent().unwrap(), parent.path());
}

#[test]
fn mk_temp_dir_in_fails_for_a_missing_parent() {
    let parent = TempDir::new().unwrap();

    let error = mk_temp_dir_in(parent.path().join("missing"), "child_").unwrap_err();
    assert!(matches!(error, FsError::Io(_)));
}

#[test]
fn mk_temp_file_creates_a_writable_file() {
    let file = mk_temp_file("rust_sak_fs_").unwrap();
    fs::write(file.path(), b"hello").unwrap();

    assert_eq!(fs::read(file.path()).unwrap(), b"hello");
}

#[test]
fn mk_temp_file_removes_the_file_on_drop() {
    let file = mk_temp_file("rust_sak_fs_").unwrap();
    let path = file.path().to_path_buf();
    drop(file);

    assert!(!path.exists());
}

#[test]
fn mk_temp_file_in_places_the_file_in_the_given_parent() {
    let parent = TempDir::new().unwrap();
    let file = mk_temp_file_in(parent.path(), "child_").unwrap();

    assert_eq!(file.path().parent().unwrap(), parent.path());
}

// --- safe path tests ---

#[test]
fn validate_entry_name_accepts_ordinary_relative_names() {
    assert_eq!(components("a.txt"), ["a.txt"]);
    assert_eq!(components("dir/a.txt"), ["dir", "a.txt"]);
    assert_eq!(components("a/b/c/d.txt"), ["a", "b", "c", "d.txt"]);
}

#[test]
fn validate_entry_name_drops_redundant_components() {
    assert_eq!(components("./a.txt"), ["a.txt"]);
    assert_eq!(components("dir//a.txt"), ["dir", "a.txt"]);
    assert_eq!(components("dir/./a.txt"), ["dir", "a.txt"]);
    assert_eq!(components("dir/"), ["dir"]);
}

#[test]
fn validate_entry_name_returns_no_components_for_the_root_itself() {
    assert!(components("").is_empty());
    assert!(components(".").is_empty());
    assert!(components("./").is_empty());
}

#[test]
fn validate_entry_name_treats_a_backslash_as_a_separator() {
    assert_eq!(components("dir\\a.txt"), ["dir", "a.txt"]);
}

#[test]
fn validate_entry_name_allows_a_colon_inside_a_component() {
    assert_eq!(components("my:file.txt"), ["my:file.txt"]);
    assert_eq!(components("dir/a:b"), ["dir", "a:b"]);
}

#[test]
fn validate_entry_name_rejects_absolute_paths() {
    assert!(matches!(
        validate_entry_name("/etc/passwd"),
        Err(FsError::IllegalPath { .. })
    ));
    assert!(matches!(
        validate_entry_name("\\windows\\system32"),
        Err(FsError::IllegalPath { .. })
    ));
}

#[test]
fn validate_entry_name_rejects_unc_style_paths() {
    assert!(matches!(
        validate_entry_name("//server/share/file"),
        Err(FsError::IllegalPath { .. })
    ));
    assert!(matches!(
        validate_entry_name("\\\\server\\share\\file"),
        Err(FsError::IllegalPath { .. })
    ));
}

#[test]
fn validate_entry_name_rejects_a_windows_drive_letter_on_every_platform() {
    for name in ["C:\\Windows", "C:/Windows", "c:relative", "Z:"] {
        assert!(
            matches!(validate_entry_name(name), Err(FsError::IllegalPath { .. })),
            "should reject {name}"
        );
    }
}

#[test]
fn validate_entry_name_rejects_parent_directory_escapes() {
    for name in ["../etc/passwd", "a/../../etc", "..", "a/..", "..\\..\\etc"] {
        assert!(
            matches!(validate_entry_name(name), Err(FsError::IllegalPath { .. })),
            "should reject {name}"
        );
    }
}

#[test]
fn validate_entry_name_rejects_reserved_windows_device_names() {
    for name in ["NUL", "nul", "CON.txt", "dir/COM1", "LPT9.tar.gz", "aux"] {
        assert!(
            matches!(validate_entry_name(name), Err(FsError::IllegalPath { .. })),
            "should reject {name}"
        );
    }
}

#[test]
fn validate_entry_name_rejects_superscript_device_names() {
    // Windows folds these onto the ASCII digits; missing them was CVE-2024-51756 in `cap-primitives`.
    for name in ["COM¹", "com²", "LPT³"] {
        assert!(
            matches!(validate_entry_name(name), Err(FsError::IllegalPath { .. })),
            "should reject {name}"
        );
    }
}

#[test]
fn validate_entry_name_allows_names_that_merely_start_like_a_device() {
    assert_eq!(components("CONFIG"), ["CONFIG"]);
    assert_eq!(components("NULL.txt"), ["NULL.txt"]);
    assert_eq!(components("COM10"), ["COM10"]);
}

#[test]
fn validate_entry_name_rejects_an_embedded_nul_byte() {
    assert!(matches!(
        validate_entry_name("a\0b.txt"),
        Err(FsError::IllegalPath { .. })
    ));
}

#[test]
fn validate_entry_name_reports_the_offending_name_and_reason() {
    let error = validate_entry_name("../escape").unwrap_err();
    let FsError::IllegalPath { path, reason } = error else {
        panic!("expected IllegalPath");
    };

    assert_eq!(path, "../escape");
    assert!(reason.contains(".."), "unexpected reason: {reason}");
}

#[test]
fn validate_symlink_target_accepts_targets_that_stay_inside() {
    validate_symlink_target(&components("link"), "file.txt").unwrap();
    validate_symlink_target(&components("dir/link"), "../other/file.txt").unwrap();
    validate_symlink_target(&components("a/b/link"), "../../a/c").unwrap();
    validate_symlink_target(&components("link"), "./nested/file").unwrap();
}

#[test]
fn validate_symlink_target_rejects_absolute_targets() {
    for target in ["/etc/passwd", "\\windows", "C:\\Windows"] {
        assert!(
            matches!(
                validate_symlink_target(&components("link"), target),
                Err(FsError::IllegalSymlink { .. })
            ),
            "should reject {target}"
        );
    }
}

#[test]
fn validate_symlink_target_rejects_climbing_above_the_root() {
    // A link at the top level has no parent to climb into, so even a single `..` escapes.
    assert!(matches!(
        validate_symlink_target(&components("link"), ".."),
        Err(FsError::IllegalSymlink { .. })
    ));
    assert!(matches!(
        validate_symlink_target(&components("dir/link"), "../../outside"),
        Err(FsError::IllegalSymlink { .. })
    ));
    assert!(matches!(
        validate_symlink_target(&components("dir/link"), "..\\..\\outside"),
        Err(FsError::IllegalSymlink { .. })
    ));
}

#[test]
fn validate_symlink_target_rejects_an_empty_or_nul_target() {
    assert!(matches!(
        validate_symlink_target(&components("link"), ""),
        Err(FsError::IllegalSymlink { .. })
    ));
    assert!(matches!(
        validate_symlink_target(&components("link"), "a\0b"),
        Err(FsError::IllegalSymlink { .. })
    ));
}

#[test]
fn validate_symlink_target_reports_both_halves() {
    let error = validate_symlink_target(&components("dir/link"), "../../outside").unwrap_err();
    let FsError::IllegalSymlink { link, target } = error else {
        panic!("expected IllegalSymlink");
    };

    assert_eq!(link, "dir/link");
    assert_eq!(target, "../../outside");
}

// --- user config tests ---

#[test]
fn config_path_joins_the_name_and_sub_path() {
    let root = TempDir::new().unwrap();

    let path = config_path(root.path(), "my-app", Path::new("themes")).unwrap();
    assert_eq!(path, root.path().join("my-app").join("themes"));
}

#[test]
fn config_path_returns_the_app_directory_for_an_empty_sub_path() {
    let root = TempDir::new().unwrap();

    let path = config_path(root.path(), "my-app", Path::new("")).unwrap();
    assert_eq!(path, root.path().join("my-app"));
}

#[test]
fn config_path_accepts_a_nested_sub_path() {
    let root = TempDir::new().unwrap();

    let path = config_path(root.path(), "my-app", Path::new("themes/dark.toml")).unwrap();
    assert_eq!(path, root.path().join("my-app").join("themes").join("dark.toml"));
}

#[test]
fn config_path_rejects_an_empty_name() {
    let root = TempDir::new().unwrap();

    assert!(matches!(
        config_path(root.path(), "", Path::new("")),
        Err(FsError::EmptyName)
    ));
}

#[test]
fn config_path_rejects_a_name_that_names_no_directory() {
    let root = TempDir::new().unwrap();

    for name in [".", "./"] {
        assert!(
            matches!(config_path(root.path(), name, Path::new("")), Err(FsError::EmptyName)),
            "should reject {name}"
        );
    }
}

#[test]
fn config_path_rejects_an_escaping_sub_path() {
    let root = TempDir::new().unwrap();

    assert!(matches!(
        config_path(root.path(), "my-app", Path::new("../../.ssh/authorized_keys")),
        Err(FsError::IllegalPath { .. })
    ));
}

#[test]
fn config_path_rejects_an_escaping_name() {
    let root = TempDir::new().unwrap();

    assert!(matches!(
        config_path(root.path(), "../evil", Path::new("")),
        Err(FsError::IllegalPath { .. })
    ));
}

#[test]
fn config_path_rejects_an_absolute_sub_path() {
    let root = TempDir::new().unwrap();

    assert!(matches!(
        config_path(root.path(), "my-app", Path::new("/etc/passwd")),
        Err(FsError::IllegalPath { .. })
    ));
}

#[test]
fn user_config_dir_lands_under_the_platform_config_directory() {
    // The only test that touches the real location — and it computes a path without creating anything.
    let expected = dirs::config_dir().expect("platform should have a config dir");

    let path = user_config_dir("rust-sak-test", "themes").unwrap();
    assert_eq!(path, expected.join("rust-sak-test").join("themes"));
}

#[test]
fn mk_config_dir_at_creates_the_directory_and_its_parents() {
    let root = TempDir::new().unwrap();

    let path = mk_config_dir_at(root.path(), "my-app", Path::new("a/b/c")).unwrap();
    assert!(path.is_dir());
    assert!(root.path().join("my-app").join("a").join("b").is_dir());
}

#[test]
fn mk_config_dir_at_succeeds_when_the_directory_already_exists() {
    let root = TempDir::new().unwrap();

    let first = mk_config_dir_at(root.path(), "my-app", Path::new("themes")).unwrap();
    let second = mk_config_dir_at(root.path(), "my-app", Path::new("themes")).unwrap();
    assert_eq!(first, second);
    assert!(second.is_dir());
}

#[test]
fn mk_config_dir_at_creates_nothing_for_an_illegal_path() {
    let root = TempDir::new().unwrap();

    assert!(mk_config_dir_at(root.path(), "my-app", Path::new("../escape")).is_err());
    assert!(!root.path().join("my-app").exists());
}

#[cfg(unix)]
#[test]
fn mk_config_dir_at_uses_mode_0755() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempDir::new().unwrap();

    let path = mk_config_dir_at(root.path(), "my-app", Path::new("themes")).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "unexpected mode {mode:o}");
}

#[test]
fn mk_config_file_at_creates_the_file_and_its_parents() {
    let root = TempDir::new().unwrap();

    let mut file = mk_config_file_at(root.path(), "my-app", Path::new("themes/dark.toml")).unwrap();
    file.write_all(b"accent = \"blue\"").unwrap();

    let path = root.path().join("my-app").join("themes").join("dark.toml");
    assert_eq!(fs::read_to_string(&path).unwrap(), "accent = \"blue\"");
}

#[test]
fn mk_config_file_at_does_not_truncate_an_existing_file() {
    let root = TempDir::new().unwrap();
    let path = config_path(root.path(), "my-app", Path::new("config.toml")).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"keep me").unwrap();

    let mut file = mk_config_file_at(root.path(), "my-app", Path::new("config.toml")).unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();

    assert_eq!(contents, "keep me");
}

#[test]
fn mk_config_file_at_rejects_an_escaping_path() {
    let root = TempDir::new().unwrap();

    assert!(matches!(
        mk_config_file_at(root.path(), "my-app", Path::new("../../.ssh/authorized_keys")),
        Err(FsError::IllegalPath { .. })
    ));
    assert!(!root.path().join("my-app").exists());
}

#[test]
fn mk_config_file_at_rejects_a_path_that_names_no_file() {
    let root = TempDir::new().unwrap();

    for file_path in ["", ".", "./"] {
        assert!(
            matches!(
                mk_config_file_at(root.path(), "my-app", Path::new(file_path)),
                Err(FsError::EmptyName)
            ),
            "should reject {file_path:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn mk_config_file_at_uses_mode_0644() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempDir::new().unwrap();

    mk_config_file_at(root.path(), "my-app", Path::new("config.toml")).unwrap();
    let path = root.path().join("my-app").join("config.toml");
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644, "unexpected mode {mode:o}");
}

#[cfg(unix)]
#[test]
fn mk_config_file_at_leaves_an_existing_files_mode_alone() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempDir::new().unwrap();
    let path = config_path(root.path(), "my-app", Path::new("config.toml")).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"secret").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

    mk_config_file_at(root.path(), "my-app", Path::new("config.toml")).unwrap();

    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "unexpected mode {mode:o}");
}

// --- list_path tests ---

/// Builds a small fixture tree and returns its root:
///
/// ```text
/// a.txt  b.LOG  c.jpg  README  nested/{d.txt, e.log, deep/f.txt}
/// ```
fn list_fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("nested/deep")).unwrap();

    for (path, contents) in [
        ("a.txt", "a"),
        ("b.LOG", "bb"),
        ("c.jpg", "ccc"),
        ("README", "dddd"),
        ("nested/d.txt", "e"),
        ("nested/e.log", "ff"),
        ("nested/deep/f.txt", "ggg"),
    ] {
        fs::write(root.join(path), contents).unwrap();
    }

    dir
}

/// Reduces a listing to file names, for comparisons that do not care about the temporary prefix.
fn names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn list_path_lists_files_one_level_deep_by_default() {
    let dir = list_fixture();

    let paths = list_path(dir.path(), &ListOptions::new()).unwrap();
    assert_eq!(names(&paths), ["README", "a.txt", "b.LOG", "c.jpg"]);
}

#[test]
fn list_path_returns_a_stable_sorted_order() {
    let dir = list_fixture();
    let options = ListOptions::new();

    assert_eq!(
        list_path(dir.path(), &options).unwrap(),
        list_path(dir.path(), &options).unwrap()
    );
}

#[test]
fn list_path_recurses_when_asked() {
    let dir = list_fixture();

    // Depth-first, so `nested`'s own entries interleave with its subtree: `d.txt`, then all of `deep`, then
    // `e.log` — `deep` sorts between them.
    let paths = list_path(dir.path(), &ListOptions::new().recursive(true)).unwrap();
    assert_eq!(
        names(&paths),
        ["README", "a.txt", "b.LOG", "c.jpg", "d.txt", "f.txt", "e.log"]
    );
}

#[test]
fn list_path_excludes_directories_by_default() {
    let dir = list_fixture();

    let paths = list_path(dir.path(), &ListOptions::new()).unwrap();
    assert!(!names(&paths).contains(&"nested".to_string()));
}

#[test]
fn list_path_includes_directories_when_asked() {
    let dir = list_fixture();

    let paths = list_path(dir.path(), &ListOptions::new().dirs(true)).unwrap();
    assert_eq!(names(&paths), ["README", "a.txt", "b.LOG", "c.jpg", "nested"]);
}

#[test]
fn list_path_can_list_only_directories() {
    let dir = list_fixture();

    let options = ListOptions::new().dirs(true).files(false).recursive(true);
    let paths = list_path(dir.path(), &options).unwrap();
    assert_eq!(names(&paths), ["nested", "deep"]);
}

#[test]
fn list_path_filters_by_extension() {
    let dir = list_fixture();

    let paths = list_path(dir.path(), &ListOptions::new().extension("txt")).unwrap();
    assert_eq!(names(&paths), ["a.txt"]);
}

#[test]
fn list_path_matches_extensions_case_insensitively() {
    let dir = list_fixture();

    let paths = list_path(dir.path(), &ListOptions::new().extension("log")).unwrap();
    assert_eq!(names(&paths), ["b.LOG"]);
}

#[test]
fn list_path_accepts_an_extension_with_a_leading_dot() {
    let dir = list_fixture();

    let with_dot = list_path(dir.path(), &ListOptions::new().extension(".txt")).unwrap();
    let without_dot = list_path(dir.path(), &ListOptions::new().extension("txt")).unwrap();
    assert_eq!(with_dot, without_dot);
}

#[test]
fn list_path_accumulates_extensions() {
    let dir = list_fixture();

    let options = ListOptions::new().extension("txt").extension("jpg");
    let paths = list_path(dir.path(), &options).unwrap();
    assert_eq!(names(&paths), ["a.txt", "c.jpg"]);
}

#[test]
fn list_path_accepts_several_extensions_at_once() {
    let dir = list_fixture();

    let options = ListOptions::new().extensions(["txt", "JPG"]);
    let paths = list_path(dir.path(), &options).unwrap();
    assert_eq!(names(&paths), ["a.txt", "c.jpg"]);
}

#[test]
fn list_path_excludes_extensionless_files_when_filtering() {
    let dir = list_fixture();

    let paths = list_path(dir.path(), &ListOptions::new().extension("txt")).unwrap();
    assert!(!names(&paths).contains(&"README".to_string()));
}

#[test]
fn list_path_never_filters_directories_by_extension() {
    let dir = list_fixture();

    let options = ListOptions::new().dirs(true).extension("txt");
    let paths = list_path(dir.path(), &options).unwrap();
    assert_eq!(names(&paths), ["a.txt", "nested"]);
}

#[test]
fn list_path_returns_an_empty_listing_for_an_empty_directory() {
    let dir = TempDir::new().unwrap();

    assert!(list_path(dir.path(), &ListOptions::new()).unwrap().is_empty());
}

#[test]
fn list_path_fails_for_a_missing_directory() {
    let dir = TempDir::new().unwrap();

    let error = list_path(dir.path().join("missing"), &ListOptions::new()).unwrap_err();
    assert!(matches!(error, FsError::Io(_)));
}

#[test]
fn list_path_keeps_the_prefix_it_was_given() {
    let dir = list_fixture();

    let paths = list_path(dir.path(), &ListOptions::new()).unwrap();
    assert!(paths.iter().all(|path| path.starts_with(dir.path())));
}

#[cfg(unix)]
#[test]
fn list_path_reports_a_symlink_as_a_file_without_following_it() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("target")).unwrap();
    fs::write(dir.path().join("target/inner.txt"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.path().join("target"), dir.path().join("link")).unwrap();

    // `link` is listed as a file and never descended into, so `inner.txt` appears exactly once — via `target`.
    let paths = list_path(dir.path(), &ListOptions::new().recursive(true)).unwrap();
    assert_eq!(names(&paths), ["link", "inner.txt"]);
}

// --- copy_files tests ---

/// Builds a source tree and returns its root:
///
/// ```text
/// a.txt (1B)  b.log (2B)  nested/{c.txt (3B), deep/d.txt (4B)}
/// ```
fn copy_fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("nested/deep")).unwrap();

    for (path, contents) in [
        ("a.txt", "a"),
        ("b.log", "bb"),
        ("nested/c.txt", "ccc"),
        ("nested/deep/d.txt", "dddd"),
    ] {
        fs::write(root.join(path), contents).unwrap();
    }

    dir
}

#[test]
fn copy_files_copies_a_single_file() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let summary = copy_files([from.path().join("a.txt")], to.path(), &CopyOptions::new()).unwrap();

    assert_eq!(
        summary,
        CopySummary {
            files: 1,
            bytes: 1,
            skipped: 0
        }
    );
    assert_eq!(fs::read_to_string(to.path().join("a.txt")).unwrap(), "a");
}

#[test]
fn copy_files_copies_a_directorys_top_level_by_default() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let summary = copy_files([from.path()], to.path(), &CopyOptions::new()).unwrap();

    assert_eq!(summary.files, 2);
    assert_eq!(summary.bytes, 3);
    assert!(to.path().join("a.txt").exists());
    assert!(!to.path().join("c.txt").exists());
}

#[test]
fn copy_files_recurses_when_asked() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let summary = copy_files([from.path()], to.path(), &CopyOptions::new().recursive(true)).unwrap();

    assert_eq!(summary.files, 4);
    assert_eq!(summary.bytes, 10);
}

#[test]
fn copy_files_flattens_by_default() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    copy_files([from.path()], to.path(), &CopyOptions::new().recursive(true)).unwrap();

    assert!(to.path().join("d.txt").exists());
    assert!(!to.path().join("nested").exists());
}

#[test]
fn copy_files_preserves_structure_when_asked() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let options = CopyOptions::new().recursive(true).preserve_structure(true);
    copy_files([from.path()], to.path(), &options).unwrap();

    assert!(to.path().join("nested/deep/d.txt").exists());
    assert_eq!(fs::read_to_string(to.path().join("nested/c.txt")).unwrap(), "ccc");
}

#[test]
fn copy_files_filters_by_extension_and_counts_the_rest_as_skipped() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let options = CopyOptions::new().recursive(true).extension("txt");
    let summary = copy_files([from.path()], to.path(), &options).unwrap();

    assert_eq!(summary.files, 3);
    assert_eq!(summary.skipped, 1);
    assert!(!to.path().join("b.log").exists());
}

#[test]
fn copy_files_accepts_several_sources() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let sources = vec![from.path().join("a.txt"), from.path().join("b.log")];
    let summary = copy_files(sources, to.path(), &CopyOptions::new()).unwrap();

    assert_eq!(summary.files, 2);
}

#[test]
fn copy_files_accepts_a_listing_directly() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let listing = list_path(from.path(), &ListOptions::new().extension("txt")).unwrap();
    let summary = copy_files(listing, to.path(), &CopyOptions::new()).unwrap();

    assert_eq!(summary.files, 1);
}

#[test]
fn copy_files_creates_a_missing_destination() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();
    let dest = to.path().join("a/b/c");

    copy_files([from.path().join("a.txt")], &dest, &CopyOptions::new()).unwrap();

    assert!(dest.join("a.txt").exists());
}

#[test]
fn copy_files_overwrites_an_existing_destination_file() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();
    fs::write(to.path().join("a.txt"), b"old contents").unwrap();

    copy_files([from.path().join("a.txt")], to.path(), &CopyOptions::new()).unwrap();

    assert_eq!(fs::read_to_string(to.path().join("a.txt")).unwrap(), "a");
}

#[test]
fn copy_files_leaves_the_sources_in_place() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    copy_files([from.path()], to.path(), &CopyOptions::new().recursive(true)).unwrap();

    assert!(from.path().join("a.txt").exists());
    assert!(from.path().join("nested/deep/d.txt").exists());
}

#[test]
fn copy_files_reports_an_empty_summary_for_no_sources() {
    let to = TempDir::new().unwrap();

    let summary = copy_files(Vec::<PathBuf>::new(), to.path(), &CopyOptions::new()).unwrap();

    assert_eq!(summary, CopySummary::default());
}

#[test]
fn copy_files_fails_for_a_missing_source() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let error = copy_files([from.path().join("missing.txt")], to.path(), &CopyOptions::new()).unwrap_err();
    assert!(matches!(error, FsError::Io(_)));
}

#[test]
fn copy_files_skips_a_file_that_would_be_copied_onto_itself() {
    let from = copy_fixture();
    let source = from.path().join("a.txt");

    let summary = copy_files([&source], from.path(), &CopyOptions::new()).unwrap();

    assert_eq!(
        summary,
        CopySummary {
            files: 0,
            bytes: 0,
            skipped: 1
        }
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), "a");
}

// --- move_files tests ---

#[test]
fn move_files_removes_the_source_file() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();
    let source = from.path().join("a.txt");

    let summary = move_files([&source], to.path(), &CopyOptions::new()).unwrap();

    assert_eq!(summary.files, 1);
    assert!(!source.exists());
    assert_eq!(fs::read_to_string(to.path().join("a.txt")).unwrap(), "a");
}

#[test]
fn move_files_moves_a_whole_tree() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let options = CopyOptions::new().recursive(true).preserve_structure(true);
    let summary = move_files([from.path()], to.path(), &options).unwrap();

    assert_eq!(summary.files, 4);
    assert!(!from.path().join("nested/deep/d.txt").exists());
    assert!(to.path().join("nested/deep/d.txt").exists());
}

#[test]
fn move_files_leaves_skipped_files_behind() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let options = CopyOptions::new().recursive(true).extension("txt");
    let summary = move_files([from.path()], to.path(), &options).unwrap();

    assert_eq!(summary.skipped, 1);
    assert!(from.path().join("b.log").exists(), "a filtered-out file must survive");
}

#[test]
fn move_files_never_removes_a_directory() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    move_files([from.path()], to.path(), &CopyOptions::new().recursive(true)).unwrap();

    assert!(from.path().is_dir());
    assert!(from.path().join("nested").is_dir());
}

#[test]
fn move_files_fails_for_a_missing_source_without_moving_anything() {
    let from = copy_fixture();
    let to = TempDir::new().unwrap();

    let sources = vec![from.path().join("missing.txt"), from.path().join("a.txt")];
    assert!(move_files(sources, to.path(), &CopyOptions::new()).is_err());
    assert!(from.path().join("a.txt").exists());
}

#[test]
fn move_files_reports_the_same_counts_as_a_copy() {
    let copied = copy_fixture();
    let moved = copy_fixture();
    let to_copy = TempDir::new().unwrap();
    let to_move = TempDir::new().unwrap();
    let options = CopyOptions::new().recursive(true);

    let copy_summary = copy_files([copied.path()], to_copy.path(), &options).unwrap();
    let move_summary = move_files([moved.path()], to_move.path(), &options).unwrap();

    assert_eq!(copy_summary, move_summary);
}

// --- extraction budget tests ---

#[test]
fn budget_allows_everything_when_no_limits_are_set() {
    let mut budget = Budget::new(&ExtractOptions::new());

    for _ in 0..1_000 {
        budget.count_entry().unwrap();
    }
    budget.reserve(u64::MAX).unwrap();
}

#[test]
fn budget_counts_entries_against_the_entry_limit() {
    let mut budget = Budget::new(&ExtractOptions::new().max_entries(2));

    budget.count_entry().unwrap();
    budget.count_entry().unwrap();

    let error = budget.count_entry().unwrap_err();
    assert!(matches!(
        error,
        FsError::LimitExceeded {
            limit: Limit::Entries,
            allowed: 2
        }
    ));
}

#[test]
fn budget_of_zero_entries_rejects_the_very_first_entry() {
    // The behaviour Go's "0 means unlimited" convention cannot express at all.
    let mut budget = Budget::new(&ExtractOptions::new().max_entries(0));

    assert!(matches!(
        budget.count_entry(),
        Err(FsError::LimitExceeded {
            limit: Limit::Entries,
            ..
        })
    ));
}

#[test]
fn budget_rejects_an_entry_larger_than_the_file_limit() {
    let mut budget = Budget::new(&ExtractOptions::new().max_file_bytes(10));

    budget.reserve(10).unwrap();

    let error = budget.reserve(11).unwrap_err();
    assert!(matches!(
        error,
        FsError::LimitExceeded {
            limit: Limit::FileBytes,
            allowed: 10
        }
    ));
}

#[test]
fn budget_rejects_entries_that_together_exceed_the_total_limit() {
    let mut budget = Budget::new(&ExtractOptions::new().max_total_bytes(10));

    budget.reserve(6).unwrap();

    let error = budget.reserve(5).unwrap_err();
    assert!(matches!(
        error,
        FsError::LimitExceeded {
            limit: Limit::TotalBytes,
            allowed: 10
        }
    ));
}

#[test]
fn budget_reserves_the_declared_size_before_anything_is_written() {
    // A header claiming 100 bytes must consume 100 bytes of budget up front, not zero.
    let mut budget = Budget::new(&ExtractOptions::new().max_total_bytes(100));

    budget.reserve(100).unwrap();
    assert_eq!(budget.used_bytes, 100);
    assert!(budget.reserve(1).is_err());
}

#[test]
fn budget_hands_back_what_an_over_declared_entry_did_not_use() {
    let mut budget = Budget::new(&ExtractOptions::new().max_total_bytes(10));

    budget.reserve(8).unwrap();
    budget.settle(8, 2);
    assert_eq!(budget.used_bytes, 2);

    // Only possible because the over-reservation was returned.
    budget.reserve(8).unwrap();
}

#[test]
fn budget_settling_an_exact_entry_changes_nothing() {
    let mut budget = Budget::new(&ExtractOptions::new().max_total_bytes(10));

    budget.reserve(5).unwrap();
    budget.settle(5, 5);

    assert_eq!(budget.used_bytes, 5);
}

#[test]
fn budget_limits_are_independent_of_one_another() {
    let options = ExtractOptions::new().max_file_bytes(5).max_total_bytes(100);
    let mut budget = Budget::new(&options);

    // Well inside the total, but too big for one file.
    assert!(matches!(
        budget.reserve(6),
        Err(FsError::LimitExceeded {
            limit: Limit::FileBytes,
            ..
        })
    ));
}

// --- archive fixture helpers ---

/// One entry to write into a test archive.
///
/// Names are written **verbatim**, which is the whole point: the hostile cases need `../escape` and `C:\evil` to
/// reach the extractor exactly as an attacker would store them.
enum TestEntry {
    File {
        name: String,
        contents: Vec<u8>,
        mode: Option<u32>,
    },
    Dir {
        name: String,
        mode: Option<u32>,
    },
    Symlink {
        name: String,
        target: String,
    },
}

impl TestEntry {
    fn file(name: &str, contents: &str) -> Self {
        TestEntry::File {
            name: name.to_string(),
            contents: contents.as_bytes().to_vec(),
            mode: None,
        }
    }

    fn file_with_mode(name: &str, contents: &str, mode: u32) -> Self {
        TestEntry::File {
            name: name.to_string(),
            contents: contents.as_bytes().to_vec(),
            mode: Some(mode),
        }
    }

    fn dir(name: &str) -> Self {
        TestEntry::Dir {
            name: name.to_string(),
            mode: None,
        }
    }

    fn dir_with_mode(name: &str, mode: u32) -> Self {
        TestEntry::Dir {
            name: name.to_string(),
            mode: Some(mode),
        }
    }

    fn symlink(name: &str, target: &str) -> Self {
        TestEntry::Symlink {
            name: name.to_string(),
            target: target.to_string(),
        }
    }
}

/// Builds a ZIP archive in memory.
///
/// `ZipWriter::start_file` stores the name it is given without sanitizing it — unlike `start_file_from_path` — which
/// is what lets these fixtures carry names no well-behaved tool would ever write.
fn build_zip(entries: &[TestEntry]) -> Vec<u8> {
    use zip::write::SimpleFileOptions;

    let mut writer = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    for entry in entries {
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        match entry {
            TestEntry::File { name, contents, mode } => {
                let options = match mode {
                    Some(mode) => options.unix_permissions(*mode),
                    None => options,
                };
                writer.start_file(name.clone(), options).unwrap();
                writer.write_all(contents).unwrap();
            }
            TestEntry::Dir { name, mode } => {
                let options = match mode {
                    Some(mode) => options.unix_permissions(*mode),
                    None => options,
                };
                writer.add_directory(name.clone(), options).unwrap();
            }
            TestEntry::Symlink { name, target } => {
                writer.add_symlink(name.clone(), target.clone(), options).unwrap();
            }
        }
    }

    writer.finish().unwrap().into_inner()
}

/// Writes archive bytes to a temporary file with the given extension and returns the handle.
fn archive_file(bytes: &[u8], extension: &str) -> NamedTempFile {
    let file = tempfile::Builder::new()
        .prefix("rust_sak_fs_")
        .suffix(extension)
        .tempfile()
        .unwrap();
    fs::write(file.path(), bytes).unwrap();

    file
}

/// A target directory with a canary file sitting beside it.
///
/// Every rejected entry is checked against this: an extractor that escapes its target lands in the parent, so the
/// parent holding exactly the canary and the target directory — and the canary still holding what it started with —
/// is what "nothing was written outside" actually means.
struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    const CANARY: &'static str = "do not touch";

    fn new() -> Self {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("target")).unwrap();
        fs::write(root.path().join("canary.txt"), Self::CANARY).unwrap();

        Self { root }
    }

    fn target(&self) -> PathBuf {
        self.root.path().join("target")
    }

    /// Asserts that nothing appeared beside the target directory and the canary is untouched.
    fn assert_nothing_escaped(&self) {
        let mut found: Vec<String> = fs::read_dir(self.root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        found.sort();

        assert_eq!(
            found,
            ["canary.txt", "target"],
            "something was written beside the target"
        );
        assert_eq!(
            fs::read_to_string(self.root.path().join("canary.txt")).unwrap(),
            Self::CANARY,
            "the canary was modified"
        );
    }
}

/// Extracts a ZIP built from `entries` into a fresh sandbox.
fn unzip_entries(entries: &[TestEntry], options: &ExtractOptions) -> (Result<ExtractSummary>, Sandbox) {
    let archive = archive_file(&build_zip(entries), ".zip");
    let sandbox = Sandbox::new();
    let result = unzip(archive.path(), sandbox.target(), options);

    (result, sandbox)
}

// --- unzip tests ---

#[test]
fn unzip_extracts_files_and_directories() {
    let entries = [
        TestEntry::dir("docs/"),
        TestEntry::file("docs/readme.txt", "hello"),
        TestEntry::file("top.txt", "hi"),
    ];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());
    let summary = result.unwrap();

    assert_eq!(summary.files, 2);
    assert_eq!(summary.directories, 1);
    assert_eq!(summary.bytes, 7);
    assert_eq!(
        fs::read_to_string(sandbox.target().join("docs/readme.txt")).unwrap(),
        "hello"
    );
    sandbox.assert_nothing_escaped();
}

#[test]
fn unzip_creates_missing_parent_directories() {
    let entries = [TestEntry::file("a/b/c/deep.txt", "deep")];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());
    result.unwrap();

    assert_eq!(
        fs::read_to_string(sandbox.target().join("a/b/c/deep.txt")).unwrap(),
        "deep"
    );
}

#[test]
fn unzip_creates_the_target_directory_when_missing() {
    let archive = archive_file(&build_zip(&[TestEntry::file("a.txt", "a")]), ".zip");
    let root = TempDir::new().unwrap();
    let target = root.path().join("does/not/exist");

    unzip(archive.path(), &target, &ExtractOptions::new()).unwrap();

    assert!(target.join("a.txt").exists());
}

#[test]
fn unzip_skips_an_entry_naming_the_target_itself() {
    let entries = [TestEntry::dir("./"), TestEntry::file("a.txt", "a")];

    let (result, _sandbox) = unzip_entries(&entries, &ExtractOptions::new());
    let summary = result.unwrap();

    assert_eq!(summary.files, 1);
    assert_eq!(summary.skipped, 1);
}

#[test]
fn unzip_overwrites_an_existing_file() {
    let archive = archive_file(&build_zip(&[TestEntry::file("a.txt", "new")]), ".zip");
    let sandbox = Sandbox::new();
    fs::write(sandbox.target().join("a.txt"), "old").unwrap();

    unzip(archive.path(), sandbox.target(), &ExtractOptions::new()).unwrap();

    assert_eq!(fs::read_to_string(sandbox.target().join("a.txt")).unwrap(), "new");
}

#[test]
fn unzip_reports_an_empty_summary_for_an_empty_archive() {
    let (result, _sandbox) = unzip_entries(&[], &ExtractOptions::new());

    assert_eq!(result.unwrap(), ExtractSummary::default());
}

#[test]
fn unzip_fails_for_a_missing_archive() {
    let sandbox = Sandbox::new();

    let error = unzip("does-not-exist.zip", sandbox.target(), &ExtractOptions::new()).unwrap_err();
    assert!(matches!(error, FsError::Io(_)));
}

#[test]
fn unzip_fails_for_a_file_that_is_not_an_archive() {
    let archive = archive_file(b"definitely not a zip file", ".zip");
    let sandbox = Sandbox::new();

    let error = unzip(archive.path(), sandbox.target(), &ExtractOptions::new()).unwrap_err();
    assert!(matches!(error, FsError::Zip(_)));
}

#[cfg(unix)]
#[test]
fn unzip_extracts_a_symlink() {
    let entries = [
        TestEntry::file("real.txt", "content"),
        TestEntry::symlink("link.txt", "real.txt"),
    ];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());
    let summary = result.unwrap();

    assert_eq!(summary.symlinks, 1);
    assert_eq!(
        fs::read_to_string(sandbox.target().join("link.txt")).unwrap(),
        "content"
    );
}

#[cfg(unix)]
#[test]
fn unzip_skips_symlinks_when_they_are_disallowed() {
    let entries = [
        TestEntry::file("real.txt", "content"),
        TestEntry::symlink("link.txt", "real.txt"),
    ];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new().symlinks(false));
    let summary = result.unwrap();

    assert_eq!(summary.symlinks, 0);
    assert_eq!(summary.skipped, 1);
    assert!(!sandbox.target().join("link.txt").exists());
}

#[cfg(unix)]
#[test]
fn unzip_allows_a_dangling_symlink() {
    // A link to something the archive never contained is legal, and common in real archives.
    let entries = [TestEntry::symlink("link.txt", "missing.txt")];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());

    assert_eq!(result.unwrap().symlinks, 1);
    assert!(sandbox.target().join("link.txt").is_symlink());
}

#[cfg(unix)]
#[test]
fn unzip_applies_the_mode_an_archive_records() {
    use std::os::unix::fs::PermissionsExt;

    let entries = [TestEntry::file_with_mode("script.sh", "#!/bin/sh\n", 0o755)];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());
    result.unwrap();

    let mode = fs::metadata(sandbox.target().join("script.sh"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755, "unexpected mode {mode:o}");
}

#[cfg(unix)]
#[test]
fn unzip_file_mode_overrides_the_archives_own() {
    use std::os::unix::fs::PermissionsExt;

    let entries = [TestEntry::file_with_mode("script.sh", "#!/bin/sh\n", 0o777)];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new().file_mode(0o600));
    result.unwrap();

    let mode = fs::metadata(sandbox.target().join("script.sh"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "unexpected mode {mode:o}");
}

// --- extraction security tests ---

/// Entry names that must be refused on every platform, whatever the archive format.
const ILLEGAL_NAMES: &[&str] = &[
    "/etc/passwd",
    "\\windows\\system32\\evil.dll",
    "../escape.txt",
    "a/../../escape.txt",
    "..\\..\\escape.txt",
    "C:\\Windows\\evil.dll",
    "C:/Windows/evil.dll",
    "//server/share/evil.dll",
    "NUL",
    "dir/COM1",
    "CON.txt",
];

#[test]
fn unzip_rejects_every_illegal_entry_name() {
    for name in ILLEGAL_NAMES {
        let (result, sandbox) = unzip_entries(&[TestEntry::file(name, "pwned")], &ExtractOptions::new());

        assert!(
            matches!(result, Err(FsError::IllegalPath { .. })),
            "should reject {name:?}, got {result:?}"
        );
        sandbox.assert_nothing_escaped();
    }
}

#[test]
fn unzip_rejects_an_illegal_name_before_writing_earlier_entries_is_undone() {
    // The good entry lands; the bad one stops the extraction. What matters is that nothing escapes.
    let entries = [
        TestEntry::file("good.txt", "fine"),
        TestEntry::file("../bad.txt", "pwned"),
    ];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());

    assert!(matches!(result, Err(FsError::IllegalPath { .. })));
    assert!(sandbox.target().join("good.txt").exists());
    sandbox.assert_nothing_escaped();
}

#[cfg(unix)]
#[test]
fn unzip_rejects_a_symlink_with_an_absolute_target() {
    let entries = [TestEntry::symlink("link", "/etc/passwd")];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());

    assert!(matches!(result, Err(FsError::IllegalSymlink { .. })), "got {result:?}");
    assert!(!sandbox.target().join("link").exists());
    sandbox.assert_nothing_escaped();
}

#[cfg(unix)]
#[test]
fn unzip_rejects_a_symlink_climbing_above_the_target() {
    for target in ["..", "../outside", "../../outside", "..\\outside"] {
        let (result, sandbox) = unzip_entries(&[TestEntry::symlink("link", target)], &ExtractOptions::new());

        assert!(
            matches!(result, Err(FsError::IllegalSymlink { .. })),
            "should reject target {target:?}, got {result:?}"
        );
        sandbox.assert_nothing_escaped();
    }
}

#[cfg(unix)]
#[test]
fn unzip_rejects_a_symlink_chained_through_another_symlink() {
    // Neither target escapes on its own reading: `a` stays put, and `a/..` climbs exactly as far as it descended.
    // But `a` is a link to the root, so `a/..` lands in the root's *parent* — an escape no lexical check can see,
    // and precisely what resolving the finished link through the anchored handle catches.
    let entries = [TestEntry::symlink("a", "."), TestEntry::symlink("b", "a/..")];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());

    assert!(matches!(result, Err(FsError::IllegalSymlink { .. })), "got {result:?}");
    assert!(
        !sandbox.target().join("b").exists(),
        "the escaping link must be removed"
    );
    sandbox.assert_nothing_escaped();
}

#[cfg(unix)]
#[test]
fn unzip_refuses_to_write_through_a_symlink_already_in_the_target() {
    // The target directory is the caller's, and may already contain anything — including a link pointing out of it.
    // The entry name `link/pwned` is impeccable; only resolving it through the anchored handle reveals the escape.
    let archive = archive_file(&build_zip(&[TestEntry::file("link/pwned", "pwned")]), ".zip");
    let sandbox = Sandbox::new();
    std::os::unix::fs::symlink("..", sandbox.target().join("link")).unwrap();

    let result = unzip(archive.path(), sandbox.target(), &ExtractOptions::new());

    assert!(matches!(result, Err(FsError::IllegalPath { .. })), "got {result:?}");
    assert!(!sandbox.root.path().join("pwned").exists());
    assert_eq!(
        fs::read_to_string(sandbox.root.path().join("canary.txt")).unwrap(),
        Sandbox::CANARY
    );
}

#[cfg(unix)]
#[test]
fn unzip_strips_the_file_type_bits_an_archive_records() {
    use std::os::unix::fs::PermissionsExt;

    // `zip` ORs `S_IFREG` into every mode it writes, so the mode read back carries type bits that must not survive.
    let entries = [TestEntry::file_with_mode("a.txt", "a", 0o640)];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());
    result.unwrap();

    let mode = fs::metadata(sandbox.target().join("a.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode & !0o777,
        0o100_000,
        "only the regular-file type bits should remain"
    );
    assert_eq!(mode & 0o777, 0o640);
}

#[test]
fn permissions_strips_setuid_setgid_and_sticky_bits() {
    // `zip`'s writer masks modes to 0o777, so these can only be reached directly — the mask itself is the guarantee
    // that an archive can never leave a setuid binary behind.
    assert_eq!(permissions(0o4755, 0o644), 0o755, "setuid must not survive");
    assert_eq!(permissions(0o2755, 0o644), 0o755, "setgid must not survive");
    assert_eq!(permissions(0o1777, 0o644), 0o777, "the sticky bit must not survive");
    assert_eq!(permissions(0o104_755, 0o644), 0o755, "type bits must not survive");
}

#[test]
fn permissions_falls_back_when_the_archive_records_nothing_usable() {
    assert_eq!(permissions(0, 0o644), 0o644);
    assert_eq!(permissions(0o170_000, 0o755), 0o755);
}

#[cfg(unix)]
#[test]
fn unzip_tightens_directory_modes_only_after_filling_them() {
    use std::os::unix::fs::PermissionsExt;

    // A directory the archive wants read-only cannot be tightened before its children are written, so the mode is
    // deferred to the end and applied deepest-first.
    let entries = [
        TestEntry::dir_with_mode("locked/", 0o500),
        TestEntry::file("locked/inside.txt", "inside"),
    ];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());
    result.unwrap();

    let locked = sandbox.target().join("locked");
    assert_eq!(fs::read_to_string(locked.join("inside.txt")).unwrap(), "inside");

    let mode = fs::metadata(&locked).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o500, "unexpected mode {mode:o}");

    // Leave it writable so the TempDir can clean itself up.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn unzip_tightens_nested_directory_modes_deepest_first() {
    use std::os::unix::fs::PermissionsExt;

    let entries = [
        TestEntry::dir_with_mode("outer/", 0o500),
        TestEntry::dir_with_mode("outer/inner/", 0o500),
        TestEntry::file("outer/inner/leaf.txt", "leaf"),
    ];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new());
    result.unwrap();

    let outer = sandbox.target().join("outer");
    let inner = outer.join("inner");
    assert_eq!(fs::metadata(&inner).unwrap().permissions().mode() & 0o777, 0o500);
    assert_eq!(fs::metadata(&outer).unwrap().permissions().mode() & 0o777, 0o500);

    fs::set_permissions(&outer, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&inner, fs::Permissions::from_mode(0o755)).unwrap();
}

// --- extraction limit tests ---

#[test]
fn unzip_enforces_the_entry_limit() {
    let entries = [
        TestEntry::file("a.txt", "a"),
        TestEntry::file("b.txt", "b"),
        TestEntry::file("c.txt", "c"),
    ];

    let (result, _sandbox) = unzip_entries(&entries, &ExtractOptions::new().max_entries(2));

    assert!(matches!(
        result,
        Err(FsError::LimitExceeded {
            limit: Limit::Entries,
            allowed: 2
        })
    ));
}

#[test]
fn unzip_counts_skipped_entries_against_the_entry_limit() {
    // The first entry names the target and extracts to nothing. It still counts, so padding an archive with
    // do-nothing entries cannot buy room under the cap.
    let entries = [TestEntry::dir("./"), TestEntry::file("a.txt", "a")];

    let (result, _sandbox) = unzip_entries(&entries, &ExtractOptions::new().max_entries(1));

    assert!(matches!(
        result,
        Err(FsError::LimitExceeded {
            limit: Limit::Entries,
            ..
        })
    ));
}

#[test]
fn unzip_rejects_the_first_entry_when_no_entries_are_allowed() {
    let (result, _sandbox) = unzip_entries(&[TestEntry::file("a.txt", "a")], &ExtractOptions::new().max_entries(0));

    assert!(matches!(
        result,
        Err(FsError::LimitExceeded {
            limit: Limit::Entries,
            allowed: 0
        })
    ));
}

#[test]
fn unzip_enforces_the_per_file_limit() {
    let entries = [TestEntry::file("big.txt", "0123456789")];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new().max_file_bytes(5));

    assert!(matches!(
        result,
        Err(FsError::LimitExceeded {
            limit: Limit::FileBytes,
            allowed: 5
        })
    ));
    assert!(!sandbox.target().join("big.txt").exists(), "nothing should be opened");
}

#[test]
fn unzip_enforces_the_total_limit_across_entries() {
    let entries = [
        TestEntry::file("a.txt", "01234"),
        TestEntry::file("b.txt", "56789"),
        TestEntry::file("c.txt", "overflow"),
    ];

    let (result, sandbox) = unzip_entries(&entries, &ExtractOptions::new().max_total_bytes(10));

    assert!(matches!(
        result,
        Err(FsError::LimitExceeded {
            limit: Limit::TotalBytes,
            allowed: 10
        })
    ));
    assert!(
        sandbox.target().join("b.txt").exists(),
        "entries within budget still land"
    );
}

#[test]
fn unzip_allows_an_archive_that_exactly_fills_its_budget() {
    let entries = [TestEntry::file("a.txt", "01234"), TestEntry::file("b.txt", "56789")];

    let options = ExtractOptions::new()
        .max_total_bytes(10)
        .max_file_bytes(5)
        .max_entries(2);
    let (result, _sandbox) = unzip_entries(&entries, &options);

    assert_eq!(result.unwrap().bytes, 10);
}

// --- tar.xz fixture and tests ---

/// Builds a TAR header, writing the name straight into the header block.
///
/// `Header::set_path` rejects several of the names these fixtures need — that is its job — so the name field
/// (the first 100 bytes of the block) is written directly and the checksum recomputed afterwards.
fn tar_header(name: &str, size: u64, mode: u32, entry_type: tar::EntryType) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_size(size);
    header.set_mode(mode);
    header.set_entry_type(entry_type);

    let bytes = name.as_bytes();
    assert!(bytes.len() <= 100, "fixture name does not fit a TAR name field: {name}");
    header.as_mut_bytes()[..bytes.len()].copy_from_slice(bytes);
    header.set_cksum();

    header
}

/// Builds an xz-compressed TAR archive in memory.
fn build_tar_xz(entries: &[TestEntry]) -> Vec<u8> {
    let mut builder = tar::Builder::new(liblzma::write::XzEncoder::new(Vec::new(), 1));

    for entry in entries {
        match entry {
            TestEntry::File { name, contents, mode } => {
                let mode = mode.unwrap_or(0o644);
                let header = tar_header(name, contents.len() as u64, mode, tar::EntryType::Regular);
                builder.append(&header, contents.as_slice()).unwrap();
            }
            TestEntry::Dir { name, mode } => {
                let mode = mode.unwrap_or(0o755);
                let header = tar_header(name, 0, mode, tar::EntryType::Directory);
                builder.append(&header, io::empty()).unwrap();
            }
            TestEntry::Symlink { name, target } => {
                let mut header = tar_header(name, 0, 0o777, tar::EntryType::Symlink);
                // The link-name field occupies bytes 157..257 of the block; writing it directly sidesteps the same
                // validation `set_path` applies, which some of these targets are meant to trip.
                let bytes = target.as_bytes();
                assert!(bytes.len() <= 100, "fixture target does not fit: {target}");
                header.as_mut_bytes()[157..157 + bytes.len()].copy_from_slice(bytes);
                header.set_cksum();
                builder.append(&header, io::empty()).unwrap();
            }
        }
    }

    builder.into_inner().unwrap().finish().unwrap()
}

/// Appends an entry of an arbitrary TAR type, for the kinds `TestEntry` deliberately cannot describe.
fn tar_xz_with_type(name: &str, entry_type: tar::EntryType) -> Vec<u8> {
    let mut builder = tar::Builder::new(liblzma::write::XzEncoder::new(Vec::new(), 1));
    let header = tar_header(name, 0, 0o644, entry_type);
    builder.append(&header, io::empty()).unwrap();

    builder.into_inner().unwrap().finish().unwrap()
}

/// Extracts a TAR.XZ built from `entries` into a fresh sandbox.
fn untar_entries(entries: &[TestEntry], options: &ExtractOptions) -> (Result<ExtractSummary>, Sandbox) {
    let archive = archive_file(&build_tar_xz(entries), ".tar.xz");
    let sandbox = Sandbox::new();
    let result = untar_xz(archive.path(), sandbox.target(), options);

    (result, sandbox)
}

#[test]
fn untar_xz_extracts_files_and_directories() {
    let entries = [
        TestEntry::dir("docs/"),
        TestEntry::file("docs/readme.txt", "hello"),
        TestEntry::file("top.txt", "hi"),
    ];

    let (result, sandbox) = untar_entries(&entries, &ExtractOptions::new());
    let summary = result.unwrap();

    assert_eq!(summary.files, 2);
    assert_eq!(summary.directories, 1);
    assert_eq!(summary.bytes, 7);
    assert_eq!(
        fs::read_to_string(sandbox.target().join("docs/readme.txt")).unwrap(),
        "hello"
    );
    sandbox.assert_nothing_escaped();
}

#[test]
fn untar_xz_round_trips_a_realistic_archive() {
    let entries = [
        TestEntry::dir("project/"),
        TestEntry::dir("project/src/"),
        TestEntry::file("project/src/main.rs", "fn main() {}"),
        TestEntry::file("project/README.md", "# project"),
    ];

    let (result, sandbox) = untar_entries(&entries, &ExtractOptions::new());
    let summary = result.unwrap();

    assert_eq!(summary.files, 2);
    assert_eq!(summary.directories, 2);
    assert_eq!(
        fs::read_to_string(sandbox.target().join("project/src/main.rs")).unwrap(),
        "fn main() {}"
    );
}

#[test]
fn untar_xz_fails_for_a_file_that_is_not_xz() {
    let archive = archive_file(b"not xz at all", ".tar.xz");
    let sandbox = Sandbox::new();

    let error = untar_xz(archive.path(), sandbox.target(), &ExtractOptions::new()).unwrap_err();
    assert!(matches!(error, FsError::Io(_)));
}

#[test]
fn untar_xz_rejects_every_illegal_entry_name() {
    for name in ILLEGAL_NAMES {
        let (result, sandbox) = untar_entries(&[TestEntry::file(name, "pwned")], &ExtractOptions::new());

        assert!(
            matches!(result, Err(FsError::IllegalPath { .. })),
            "should reject {name:?}, got {result:?}"
        );
        sandbox.assert_nothing_escaped();
    }
}

#[test]
fn untar_xz_skips_entries_a_filesystem_cannot_safely_reproduce() {
    for entry_type in [
        tar::EntryType::Fifo,
        tar::EntryType::Char,
        tar::EntryType::Block,
        tar::EntryType::Link,
    ] {
        let archive = archive_file(&tar_xz_with_type("device", entry_type), ".tar.xz");
        let sandbox = Sandbox::new();

        let summary = untar_xz(archive.path(), sandbox.target(), &ExtractOptions::new()).unwrap();

        assert_eq!(summary.skipped, 1, "{entry_type:?} should be skipped");
        assert_eq!(summary.files, 0);
        assert!(
            !sandbox.target().join("device").exists(),
            "{entry_type:?} must not be created"
        );
    }
}

#[cfg(unix)]
#[test]
fn untar_xz_extracts_a_symlink_from_the_header() {
    // Unlike ZIP, TAR keeps the target in the header rather than the entry's stream.
    let entries = [
        TestEntry::file("real.txt", "content"),
        TestEntry::symlink("link.txt", "real.txt"),
    ];

    let (result, sandbox) = untar_entries(&entries, &ExtractOptions::new());

    assert_eq!(result.unwrap().symlinks, 1);
    assert_eq!(
        fs::read_to_string(sandbox.target().join("link.txt")).unwrap(),
        "content"
    );
}

#[cfg(unix)]
#[test]
fn untar_xz_rejects_an_escaping_symlink_target() {
    for target in ["/etc/passwd", "..", "../../outside"] {
        let (result, sandbox) = untar_entries(&[TestEntry::symlink("link", target)], &ExtractOptions::new());

        assert!(
            matches!(result, Err(FsError::IllegalSymlink { .. })),
            "should reject target {target:?}, got {result:?}"
        );
        sandbox.assert_nothing_escaped();
    }
}

#[cfg(unix)]
#[test]
fn untar_xz_rejects_a_symlink_chained_through_another_symlink() {
    let entries = [TestEntry::symlink("a", "."), TestEntry::symlink("b", "a/..")];

    let (result, sandbox) = untar_entries(&entries, &ExtractOptions::new());

    assert!(matches!(result, Err(FsError::IllegalSymlink { .. })), "got {result:?}");
    sandbox.assert_nothing_escaped();
}

#[cfg(unix)]
#[test]
fn untar_xz_strips_setuid_setgid_and_sticky_bits() {
    use std::os::unix::fs::PermissionsExt;

    // TAR is the one fixture format that can express these: `zip`'s writer masks modes to 0o777.
    let entries = [
        TestEntry::file_with_mode("setuid", "x", 0o4755),
        TestEntry::file_with_mode("setgid", "x", 0o2755),
        TestEntry::file_with_mode("sticky", "x", 0o1755),
    ];

    let (result, sandbox) = untar_entries(&entries, &ExtractOptions::new());
    result.unwrap();

    for name in ["setuid", "setgid", "sticky"] {
        let mode = fs::metadata(sandbox.target().join(name)).unwrap().permissions().mode();
        assert_eq!(mode & 0o7000, 0, "{name} kept a special bit: {mode:o}");
        assert_eq!(mode & 0o777, 0o755, "{name} lost its permission bits: {mode:o}");
    }
}

#[test]
fn untar_xz_enforces_the_same_limits_as_unzip() {
    let entries = [TestEntry::file("a.txt", "01234"), TestEntry::file("b.txt", "56789")];

    let (result, _sandbox) = untar_entries(&entries, &ExtractOptions::new().max_total_bytes(6));
    assert!(matches!(
        result,
        Err(FsError::LimitExceeded {
            limit: Limit::TotalBytes,
            ..
        })
    ));

    let (result, _sandbox) = untar_entries(&entries, &ExtractOptions::new().max_entries(1));
    assert!(matches!(
        result,
        Err(FsError::LimitExceeded {
            limit: Limit::Entries,
            ..
        })
    ));
}

#[test]
fn unzip_rejects_an_entry_name_with_an_embedded_nul() {
    // Only ZIP can carry this: a TAR name field is NUL-terminated, so the byte would end the name rather than sit
    // inside it.
    let (result, sandbox) = unzip_entries(&[TestEntry::file("evil\0.txt", "pwned")], &ExtractOptions::new());

    assert!(matches!(result, Err(FsError::IllegalPath { .. })), "got {result:?}");
    sandbox.assert_nothing_escaped();
}

// --- 7z fixture and tests ---

/// Builds a 7z archive in memory.
///
/// `sevenz-rust2`'s writer cannot express the p7zip `0x8000` attribute form, so these fixtures cover files and
/// directories only — symbolic links and Unix modes are covered by unit-testing [`super::un7zip::classify`] directly.
fn build_7z(entries: &[TestEntry]) -> Vec<u8> {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};

    let mut writer = ArchiveWriter::new(io::Cursor::new(Vec::new())).unwrap();
    for entry in entries {
        match entry {
            TestEntry::File { name, contents, .. } => {
                writer
                    .push_archive_entry(ArchiveEntry::new_file(name), Some(contents.as_slice()))
                    .unwrap();
            }
            TestEntry::Dir { name, .. } => {
                writer
                    .push_archive_entry(ArchiveEntry::new_directory(name), None::<&[u8]>)
                    .unwrap();
            }
            TestEntry::Symlink { .. } => panic!("the 7z writer cannot express symbolic links"),
        }
    }

    writer.finish().unwrap().into_inner()
}

/// Extracts a 7z built from `entries` into a fresh sandbox.
fn un7zip_entries(entries: &[TestEntry], options: &ExtractOptions) -> (Result<ExtractSummary>, Sandbox) {
    let archive = archive_file(&build_7z(entries), ".7z");
    let sandbox = Sandbox::new();
    let result = un7zip(archive.path(), sandbox.target(), options);

    (result, sandbox)
}

#[test]
fn un7zip_extracts_files_and_directories() {
    let entries = [
        TestEntry::dir("docs"),
        TestEntry::file("docs/readme.txt", "hello"),
        TestEntry::file("top.txt", "hi"),
    ];

    let (result, sandbox) = un7zip_entries(&entries, &ExtractOptions::new());
    let summary = result.unwrap();

    assert_eq!(summary.files, 2);
    assert_eq!(summary.directories, 1);
    assert_eq!(summary.bytes, 7);
    assert_eq!(
        fs::read_to_string(sandbox.target().join("docs/readme.txt")).unwrap(),
        "hello"
    );
    sandbox.assert_nothing_escaped();
}

#[test]
fn un7zip_fails_for_a_file_that_is_not_an_archive() {
    let archive = archive_file(b"not a 7z archive", ".7z");
    let sandbox = Sandbox::new();

    let error = un7zip(archive.path(), sandbox.target(), &ExtractOptions::new()).unwrap_err();
    assert!(matches!(error, FsError::SevenZ(_)));
}

#[test]
fn un7zip_rejects_every_illegal_entry_name() {
    for name in ILLEGAL_NAMES {
        let (result, sandbox) = un7zip_entries(&[TestEntry::file(name, "pwned")], &ExtractOptions::new());

        assert!(
            matches!(result, Err(FsError::IllegalPath { .. })),
            "should reject {name:?}, got {result:?}"
        );
        sandbox.assert_nothing_escaped();
    }
}

#[test]
fn un7zip_reports_our_error_rather_than_the_walks_own() {
    // Stopping `for_each_entries` early makes it report its own, less specific complaint; ours must win.
    let entries = [TestEntry::file("a.txt", "a"), TestEntry::file("b.txt", "b")];

    let (result, _sandbox) = un7zip_entries(&entries, &ExtractOptions::new().max_entries(1));

    assert!(
        matches!(
            result,
            Err(FsError::LimitExceeded {
                limit: Limit::Entries,
                ..
            })
        ),
        "got {result:?}"
    );
}

#[test]
fn un7zip_enforces_the_byte_limits() {
    let entries = [TestEntry::file("a.txt", "01234"), TestEntry::file("b.txt", "56789")];

    let (result, _sandbox) = un7zip_entries(&entries, &ExtractOptions::new().max_total_bytes(6));

    assert!(matches!(
        result,
        Err(FsError::LimitExceeded {
            limit: Limit::TotalBytes,
            ..
        })
    ));
}

#[test]
fn classify_reads_a_unix_mode_out_of_the_p7zip_attribute_word() {
    // p7zip sets 0x8000 to declare that the high half is an `st_mode`: here a 0o644 regular file.
    let attributes = (0o100_644 << 16) | 0x8000;

    assert_eq!(
        classify(false, true, attributes),
        (EntryKind::File, Some(0o100_644)),
        "the raw st_mode should come through; masking happens later"
    );
}

#[test]
fn classify_recognises_a_symlink_from_the_attribute_word() {
    let attributes = (0o120_777 << 16) | 0x8000;

    assert_eq!(classify(false, true, attributes), (EntryKind::Symlink, Some(0o120_777)));
}

#[test]
fn classify_finds_no_mode_and_no_symlink_without_the_unix_flag() {
    // What a Windows-written or `sevenz-rust2`-written archive looks like: attributes present, but not p7zip's form.
    assert_eq!(classify(false, true, 0o120_777 << 16), (EntryKind::File, None));
    assert_eq!(classify(false, false, 0), (EntryKind::File, None));
}

#[test]
fn classify_treats_a_directory_as_a_directory_whatever_the_attributes_say() {
    let attributes = (0o120_777 << 16) | 0x8000;

    let (kind, _) = classify(true, true, attributes);
    assert_eq!(kind, EntryKind::Dir, "a link bit must not turn a directory into a link");
}

#[test]
fn classify_output_survives_the_permission_mask() {
    // The two halves together: `classify` hands back a raw st_mode, `permissions` reduces it to what lands on disk.
    let (_, mode) = classify(false, true, (0o104_755 << 16) | 0x8000);

    assert_eq!(permissions(mode.unwrap(), 0o644), 0o755);
}

// --- extract dispatch tests ---

#[test]
fn extract_dispatches_on_the_file_extension() {
    let entries = [TestEntry::file("a.txt", "hello")];

    for (bytes, extension) in [
        (build_zip(&entries), ".zip"),
        (build_tar_xz(&entries), ".tar.xz"),
        (build_7z(&entries), ".7z"),
    ] {
        let archive = archive_file(&bytes, extension);
        let sandbox = Sandbox::new();

        let summary = extract(archive.path(), sandbox.target(), &ExtractOptions::new())
            .unwrap_or_else(|err| panic!("{extension} should extract: {err}"));

        assert_eq!(summary.files, 1, "{extension}");
        assert_eq!(fs::read_to_string(sandbox.target().join("a.txt")).unwrap(), "hello");
    }
}

#[test]
fn extract_matches_extensions_case_insensitively() {
    let archive = archive_file(&build_zip(&[TestEntry::file("a.txt", "hi")]), ".ZIP");
    let sandbox = Sandbox::new();

    assert_eq!(
        extract(archive.path(), sandbox.target(), &ExtractOptions::new())
            .unwrap()
            .files,
        1
    );
}

#[test]
fn extract_recognises_the_txz_abbreviation() {
    let archive = archive_file(&build_tar_xz(&[TestEntry::file("a.txt", "hi")]), ".txz");
    let sandbox = Sandbox::new();

    assert_eq!(
        extract(archive.path(), sandbox.target(), &ExtractOptions::new())
            .unwrap()
            .files,
        1
    );
}

#[test]
fn extract_rejects_an_unknown_extension() {
    let sandbox = Sandbox::new();

    for name in ["archive.rar", "archive.tar.gz", "archive.xz", "archive", "archive.tar"] {
        let archive = sandbox.root.path().join(name);
        fs::write(&archive, b"whatever").unwrap();

        let error = extract(&archive, sandbox.target(), &ExtractOptions::new()).unwrap_err();
        assert!(
            matches!(error, FsError::UnknownArchiveFormat),
            "should reject {name}, got {error:?}"
        );
    }
}

#[test]
fn extract_passes_options_through_to_the_format() {
    let archive = archive_file(&build_zip(&[TestEntry::file("big.txt", "0123456789")]), ".zip");
    let sandbox = Sandbox::new();

    let result = extract(
        archive.path(),
        sandbox.target(),
        &ExtractOptions::new().max_file_bytes(5),
    );

    assert!(matches!(
        result,
        Err(FsError::LimitExceeded {
            limit: Limit::FileBytes,
            ..
        })
    ));
}

// --- error tests ---

#[test]
fn errors_display_usefully() {
    let cases: Vec<(FsError, &str)> = vec![
        (FsError::EmptyName, "empty"),
        (FsError::NoConfigDir, "configuration directory"),
        (FsError::UnknownArchiveFormat, "unknown archive format"),
        (
            FsError::IllegalPath {
                path: "../x".into(),
                reason: "nope".into(),
            },
            "../x",
        ),
        (
            FsError::IllegalSymlink {
                link: "l".into(),
                target: "/etc".into(),
            },
            "/etc",
        ),
        (
            FsError::DeclaredSizeMismatch {
                entry: "e".into(),
                declared: 7,
            },
            "7",
        ),
        (
            FsError::LimitExceeded {
                limit: Limit::TotalBytes,
                allowed: 99,
            },
            "99",
        ),
    ];

    for (error, expected) in cases {
        let rendered = error.to_string();
        assert!(rendered.contains(expected), "{rendered:?} should mention {expected:?}");
    }
}

#[test]
fn limit_displays_which_ceiling_was_hit() {
    assert_eq!(Limit::TotalBytes.to_string(), "total bytes");
    assert_eq!(Limit::FileBytes.to_string(), "file bytes");
    assert_eq!(Limit::Entries.to_string(), "entries");
}

#[test]
fn wrapper_errors_expose_their_source() {
    use std::error::Error;

    let io = FsError::Io(io::Error::other("boom"));
    assert!(io.source().is_some());

    assert!(FsError::EmptyName.source().is_none());
    assert!(FsError::UnknownArchiveFormat.source().is_none());
}

#[test]
fn io_errors_convert_into_fs_errors() {
    let error: FsError = io::Error::other("boom").into();

    assert!(matches!(error, FsError::Io(_)));
}
