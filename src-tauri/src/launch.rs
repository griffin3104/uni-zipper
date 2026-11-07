//! 「プログラムから開く」(Finder / エクスプローラー)で渡されたファイルの受け取り。
//!
//! - macOS: アプリが「ファイルを開く」イベント(`RunEvent::Opened`)で、`file://` のURLを受け取る。
//! - Windows / Linux: 起動時のコマンドライン引数に、ファイルのパスが渡される。
//!
//! どちらも、ドラッグ&ドロップと同じ扱いにする(画面側で `classify_drop` に渡す)。

use std::path::PathBuf;

use tauri::Url;

/// 起動時の引数(実行ファイル自身を除いたもの)から、開くファイルのパスを取り出す。
///
/// `-` で始まるもの(オプション。macOSの `-psn_...` など)と、存在しないパスは、無視する。
pub fn paths_from_args(args: impl IntoIterator<Item = String>) -> Vec<PathBuf> {
    args.into_iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect()
}

/// 「ファイルを開く」イベントのURLから、ファイルのパスを取り出す(`file://` 以外は無視する)。
/// 使うのはmacOSだけ(他のOSでは、未使用の警告を出さない)。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn paths_from_urls(urls: &[Url]) -> Vec<PathBuf> {
    urls.iter().filter_map(|url| url.to_file_path().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn 引数から存在するパスだけを取り出す() {
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("資料 1.zip");
        fs::write(&zip, b"x").unwrap();
        let args = vec![
            "-psn_0_12345".to_string(),
            "--flag".to_string(),
            zip.to_string_lossy().into_owned(),
            dir.path().join("missing.zip").to_string_lossy().into_owned(),
        ];
        assert_eq!(paths_from_args(args), [zip]);
    }

    #[test]
    fn 引数が無ければ空() {
        assert!(paths_from_args(Vec::<String>::new()).is_empty());
    }

    #[test]
    fn urlからパスを取り出す() {
        // 日本語や空白は、URLではパーセントエンコードされている
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("議事録 第1回.zip");
        let url = Url::from_file_path(&zip).unwrap();
        assert!(url.as_str().contains("%"), "{url}");
        assert_eq!(paths_from_urls(&[url]), [zip]);
    }

    #[test]
    fn file以外のurlは無視する() {
        let web = Url::parse("https://example.com/a.zip").unwrap();
        assert!(paths_from_urls(&[web]).is_empty());
    }
}
