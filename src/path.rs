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
