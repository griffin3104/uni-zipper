//! 圧縮・解凍時に除外するOS由来の不要ファイルの判定。
//!
//! 除外対象(docs/機能要件.md 3.4 / 4.3):
//! `.DS_Store`、`__MACOSX`、`Thumbs.db`、`desktop.ini`


/// 除外する名前。大文字小文字は区別しない(Windowsは区別しないため)。
const EXCLUDED_NAMES: &[&str] = &[".DS_Store", "__MACOSX", "Thumbs.db", "desktop.ini"];

/// ファイル名またはフォルダ名1つが、除外対象か。
pub fn is_excluded_name(name: &str) -> bool {
    EXCLUDED_NAMES
        .iter()
        .any(|excluded| excluded.eq_ignore_ascii_case(name))
}

/// Zip内のパス(`/` または `\` 区切り)の、どこかの階層が除外対象か。
///
/// `__MACOSX/foo/bar.txt` のように、除外フォルダの配下も除外される。
pub fn is_excluded_path(path: &str) -> bool {
    path.split(['/', '\\']).any(is_excluded_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 除外対象の名前を判定できる() {
        for name in [".DS_Store", "__MACOSX", "Thumbs.db", "desktop.ini"] {
            assert!(is_excluded_name(name), "{name}");
        }
    }

    #[test]
    fn 大文字小文字は区別しない() {
        assert!(is_excluded_name("thumbs.db"));
        assert!(is_excluded_name("THUMBS.DB"));
        assert!(is_excluded_name("Desktop.ini"));
        assert!(is_excluded_name(".ds_store"));
        assert!(is_excluded_name("__macosx"));
    }

    #[test]
    fn 似た名前は除外しない() {
        for name in ["DS_Store", ".DS_Store.txt", "my_Thumbs.db", "desktop.ini.bak", "MACOSX", ""] {
            assert!(!is_excluded_name(name), "{name}");
        }
    }

    #[test]
    fn パスのどの階層でも除外される() {
        assert!(is_excluded_path(".DS_Store"));
        assert!(is_excluded_path("a/b/.DS_Store"));
        assert!(is_excluded_path("a\\b\\Thumbs.db"));
        assert!(is_excluded_path("__MACOSX/a/b.txt"));
        assert!(is_excluded_path("a/__MACOSX/b.txt"));
        assert!(is_excluded_path("a/desktop.ini"));
    }

    #[test]
    fn 通常のパスは除外しない() {
        assert!(!is_excluded_path("a/b/c.txt"));
        assert!(!is_excluded_path("日本語/ファイル.txt"));
        assert!(!is_excluded_path("MACOSX/a.txt"));
        assert!(!is_excluded_path(""));
    }
}
