//! Path policy for authored, bank-relative paths.
//!
//! Questions reference other files by path: `file:` partials and local images.
//! Those paths are authored, resolved against a base directory, and must not be
//! able to reach outside it. The two rules that decide what is acceptable live
//! here, rather than in the stages that apply them, because the parser, the
//! exporters and the CLI all have to agree on them.

use std::path::{Component, Path};

/// Whether `path` would resolve outside the base directory it is joined to.
///
/// A path is safe only when every component is an ordinary name (`.` is
/// tolerated). That rejects `..` components, absolute paths, and Windows
/// prefixes such as `C:\` — all of which escape: `Path::join` *discards* the
/// base entirely when given an absolute path, so rejecting `..` alone is not
/// enough.
///
/// Deliberately expressed over [`Path::components`] rather than by splitting on
/// `/`, so platform separator rules are applied by the standard library instead
/// of guessed at here.
#[must_use]
pub fn escapes_dir(path: &str) -> bool {
    Path::new(path)
        .components()
        .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
}

/// The directory part of a bank-relative `path`, or `""` for a top-level file.
///
/// Bank-relative paths always use `/`, whatever the host platform, because
/// they are built from directory walks rather than read from the filesystem —
/// so this splits on `/` rather than going through [`std::path::Path`].
#[must_use]
pub fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Join a bank-relative directory and a path inside it.
///
/// A `.` or empty directory yields the path unchanged, so `./topics` and
/// `topics` name the same place — the same rule [`nests`] compares by, rather
/// than gluing a `./` prefix onto every path below it.
#[must_use]
pub fn join_dir(dir: &str, path: &str) -> String {
    match dir.trim_end_matches('/') {
        "" | "." => path.to_owned(),
        dir => format!("{dir}/{path}"),
    }
}

/// The meaningful name components of a bank-relative directory.
///
/// Drops `.` and any root/prefix component so two spellings of the same folder
/// compare equal. Callers reject escaping paths first, so dropping `..` here
/// cannot mask one.
fn dir_parts(dir: &str) -> Vec<&std::ffi::OsStr> {
    Path::new(dir)
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect()
}

/// Whether two directories would draw from the same files: the same folder, or
/// one nested inside the other.
///
/// Compared by path component, not by string prefix, so `./topics`,
/// `topics//trees` and `topics/./trees` are recognised as the folders they
/// actually name. String matching misses all three.
///
/// Note two empty paths nest. Callers reject an empty directory *before*
/// checking overlap, so that case never reaches here.
#[must_use]
pub fn nests(one: &str, other: &str) -> bool {
    let (one, other) = (dir_parts(one), dir_parts(other));
    one.starts_with(&other) || other.starts_with(&one)
}

/// Whether an image URL is a local file path (rather than an absolute URL).
///
/// Shared by the parser (which rebases a partial's local images) and the Canvas
/// exporter (which bundles them). Absolute URLs — `scheme://`, protocol-relative
/// `//`, root-relative `/`, and `data:` — are not local.
pub(crate) fn is_local_image(url: &str) -> bool {
    !(url.contains("://")
        || url.starts_with("//")
        || url.starts_with('/')
        || url.starts_with("data:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Ordinary relative paths are accepted; `..`, absolute paths and Windows
    /// prefixes are all treated as escaping.
    fn escapes_dir_rejects_every_non_normal_component() {
        for path in [
            "../x.png",
            "a/../../x.png",
            "a/../x.png",
            "..",
            "/etc/passwd",
        ] {
            assert!(escapes_dir(path), "{path} should escape");
        }
        for path in ["x.png", "a/b/x.png", "..dots.png", "a/..b/x.png", "./x.png"] {
            assert!(!escapes_dir(path), "{path} should not escape");
        }
    }

    #[test]
    #[expect(
        clippy::join_absolute_paths,
        reason = "the discarded base is precisely the footgun this test pins"
    )]
    /// An absolute path escapes because `Path::join` discards the base for one,
    /// so a `file:` partial naming one would read outside the question tree.
    fn escapes_dir_rejects_absolute_paths_that_join_would_honour() {
        let base = Path::new("/questions");
        assert_eq!(base.join("/etc/passwd"), Path::new("/etc/passwd"));
        assert!(escapes_dir("/etc/passwd"));
    }

    #[test]
    /// Folders that name the same place nest, however they are spelled, and
    /// a shared prefix that is not a path boundary does not.
    fn nests_compares_path_components_not_strings() {
        for (one, other) in [
            ("topics", "topics"),
            ("topics/", "topics"),
            ("./topics", "topics"),
            ("topics//trees", "topics/trees"),
            ("topics/./trees", "topics/trees"),
            ("topics", "topics/trees"),
            ("topics/trees", "topics"),
        ] {
            assert!(nests(one, other), "{one} should nest with {other}");
        }
        for (one, other) in [
            ("topics/tree", "topics/trees"),
            ("topics/trees", "topics/graphs"),
            ("a/b", "b/a"),
        ] {
            assert!(!nests(one, other), "{one} should not nest with {other}");
        }
    }

    #[test]
    /// A path below a folder carries it; a `.` or empty folder adds nothing,
    /// so the two spellings of "here" do not produce two different paths for
    /// the same file.
    fn join_dir_prefixes_only_a_real_directory() {
        assert_eq!(join_dir("topics", "q1.md"), "topics/q1.md");
        assert_eq!(join_dir("topics/trees", "q1.md"), "topics/trees/q1.md");
        assert_eq!(join_dir("topics/", "q1.md"), "topics/q1.md");
        assert_eq!(join_dir("./topics", "q1.md"), "./topics/q1.md");
        assert_eq!(join_dir(".", "q1.md"), "q1.md");
        assert_eq!(join_dir("", "q1.md"), "q1.md");
    }

    #[test]
    /// The directory part is everything before the last `/`; a top-level file
    /// has none, and nested paths keep their full parent.
    fn parent_dir_splits_on_the_last_separator() {
        assert_eq!(parent_dir("a/b/c.md"), "a/b");
        assert_eq!(parent_dir("a/c.md"), "a");
        assert_eq!(parent_dir("c.md"), "");
        assert_eq!(parent_dir(""), "");
    }

    #[test]
    /// Only relative paths count as local; every URL scheme is rejected.
    fn is_local_image_rejects_urls_accepts_relative() {
        for url in [
            "http://x/y.png",
            "https://x/y.png",
            "//x/y.png",
            "/root/y.png",
            "data:image/png;base64,AAAA",
        ] {
            assert!(!is_local_image(url), "{url} should be non-local");
        }
        assert!(is_local_image("diagram.png"));
        assert!(is_local_image("sub/diagram.png"));
    }
}
