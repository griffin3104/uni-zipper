//! アプリ共通のエラー型。
//!
//! メッセージはそのまま画面のエラーダイアログに表示できる日本語にする。


use std::path::PathBuf;

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("ファイルの読み書きに失敗しました: {0}")]
    Io(#[from] std::io::Error),

    #[error("シンボリックリンクは追加できません: {}", .0.display())]
    SymlinkRejected(PathBuf),

    #[error("パスが正しくありません: {}", .0.display())]
    InvalidPath(PathBuf),

    #[error("「{0}」が見つかりません")]
    ItemNotFound(String),

    #[error("Zipファイルとして読み込めません: {0}")]
    InvalidZip(String),

    #[error("ファイル名の文字コードが決まっていません")]
    CharsetNotResolved,

    #[error("パスワードが必要です")]
    PasswordRequired,

    #[error("パスワードが違います")]
    WrongPassword,

    #[error("キャンセルされました")]
    Cancelled,

    #[error("別の処理を実行中です")]
    Busy,

    #[error("圧縮するものがありません")]
    EmptyWorkDir,
}

impl AppError {
    /// フロントエンドが、エラーの種類で処理を分けるための識別子。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::SymlinkRejected(_) => "symlinkRejected",
            Self::InvalidPath(_) => "invalidPath",
            Self::ItemNotFound(_) => "itemNotFound",
            Self::InvalidZip(_) => "invalidZip",
            Self::CharsetNotResolved => "charsetNotResolved",
            Self::PasswordRequired => "passwordRequired",
            Self::WrongPassword => "wrongPassword",
            Self::Cancelled => "cancelled",
            Self::Busy => "busy",
            Self::EmptyWorkDir => "emptyWorkDir",
        }
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

/// Tauriのコマンドからフロントエンドへ返すため、`{ kind, message }` としてシリアライズする。
/// `kind` で処理を分け、`message` はそのままダイアログに表示できる。
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn エラーはkindとmessageのjsonになる() {
        let json = serde_json::to_value(AppError::WrongPassword).unwrap();
        assert_eq!(json, serde_json::json!({ "kind": "wrongPassword", "message": "パスワードが違います" }));

        let json = serde_json::to_value(AppError::SymlinkRejected(PathBuf::from("/a/link"))).unwrap();
        assert_eq!(json["kind"], "symlinkRejected");
        assert!(json["message"].as_str().unwrap().contains("/a/link"));
    }
}
