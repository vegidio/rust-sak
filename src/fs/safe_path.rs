use super::{FsError, Result};

/// Windows device names that are reserved regardless of extension or directory.
///
/// Opening one of these by name talks to a device rather than creating a file, so an archive containing `NUL` or
/// `COM1` is hostile on Windows no matter how innocent it looks on Unix. The superscript `COM¹`/`LPT²` forms are
/// included because Windows folds them onto the ASCII digits — the same trick behind CVE-2024-51756 in
/// `cap-primitives`.
const RESERVED_STEMS: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CLOCK$", //
    "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", //
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Splits an archive entry name into path components, rejecting anything that could escape the extraction root.
///
/// Every rule below is applied on **every platform**, so an archive that is dangerous on Windows is refused on Linux
/// and macOS too. That costs a few legitimate-on-Unix names and buys a single, testable notion of "safe entry" rather
/// than one that shifts with the build target.
///
/// Rejected: absolute paths (`/etc/passwd`, `\windows`), UNC paths (`//server/share` — absolute, so covered by the
/// same rule), a leading drive letter (`C:\`, `C:/`, `C:foo`), any `..` component, [reserved Windows device
/// names](RESERVED_STEMS), and embedded NUL bytes.
///
/// Both `/` and `\` are treated as separators, which is what makes `..\..\etc` a rejected escape rather than a file
/// with an odd name. `.` and empty components are dropped. A colon is otherwise allowed anywhere, so `my:file.txt`
/// survives; only the two-character drive prefix at position 0 is refused.
///
/// An entry naming the root itself (`""`, `"."`, `"./"`) yields an **empty** component list rather than an error —
/// archives routinely contain such an entry, and the caller skips it.
///
/// # Errors
///
/// Returns [`FsError::IllegalPath`] describing the offending name and the rule it broke.
pub(super) fn validate_entry_name(name: &str) -> Result<Vec<&str>> {
    let reject = |reason: &str| {
        Err(FsError::IllegalPath {
            path: name.to_string(),
            reason: reason.to_string(),
        })
    };

    if let Some(reason) = unsafe_prefix(name) {
        return reject(reason);
    }

    let mut components = Vec::new();
    for component in name.split(['/', '\\']) {
        match component {
            "" | "." => continue,
            ".." => return reject("contains a `..` component"),
            _ => {}
        }

        if is_reserved_device_name(component) {
            return reject("contains a reserved Windows device name");
        }

        components.push(component);
    }

    Ok(components)
}

/// Reports why `path` cannot be trusted as a relative, contained path, or `None` if its prefix is fine.
///
/// Shared by both validators below. An entry name and a symlink target are rejected for the same three reasons, and
/// a rule that landed in only one of them is exactly the failure this module exists to prevent — so the rule set
/// lives here once rather than being spelled out twice.
fn unsafe_prefix(path: &str) -> Option<&'static str> {
    if path.contains('\0') {
        return Some("contains an embedded NUL byte");
    }

    if path.starts_with('/') || path.starts_with('\\') {
        return Some("is an absolute path");
    }

    // A drive prefix is exactly one ASCII letter followed by a colon, at the very start. Checking bytes is safe here:
    // a multi-byte UTF-8 sequence never contains an ASCII byte, so this cannot split a character.
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Some("starts with a Windows drive letter");
    }

    None
}

/// Checks that a symbolic link's target cannot leave the extraction root, judging by the text of the target alone.
///
/// `link_components` is the already-validated name of the link itself, as returned by [`validate_entry_name`]; the
/// target is resolved relative to the link's **parent**, exactly as the OS would resolve it. Walking the target
/// component by component and tracking the remaining depth catches `../../etc/passwd` while still permitting a link
/// that climbs and descends within the tree, such as `../sibling/file`.
///
/// This is a purely lexical check and deliberately not the only one. It cannot see links planted by earlier entries —
/// `link` → `.` followed by `link/escape` → `..` is contained at every individual step — so the extractor re-checks
/// each created link through the anchored directory handle as well. This function's job is to reject the obvious
/// cases cheaply, before anything touches the filesystem.
///
/// # Errors
///
/// Returns [`FsError::IllegalSymlink`] if the target is absolute, empty, or climbs above the root.
pub(super) fn validate_symlink_target(link_components: &[&str], target: &str) -> Result<()> {
    let reject = || {
        Err(FsError::IllegalSymlink {
            link: link_components.join("/"),
            target: target.to_string(),
        })
    };

    // An empty target is symlink-specific — there is no such thing as a link pointing nowhere — so it is checked
    // here rather than in the shared prefix rules.
    if target.is_empty() || unsafe_prefix(target).is_some() {
        return reject();
    }

    // The link resolves from the directory holding it, so it starts one level shallower than its own name.
    let mut depth = link_components.len().saturating_sub(1);
    for component in target.split(['/', '\\']) {
        match component {
            "" | "." => continue,
            ".." => match depth.checked_sub(1) {
                Some(next) => depth = next,
                None => return reject(),
            },
            _ => depth += 1,
        }
    }

    Ok(())
}

/// Reports whether `component` names a reserved Windows device.
///
/// Windows matches these on the stem — everything before the first `.` — so `NUL.txt` is just as much the null device
/// as `NUL`, and trailing spaces are ignored too.
fn is_reserved_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component).trim_end();
    RESERVED_STEMS
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}
