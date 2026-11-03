//! アプリ共通のエラー型。
//!
//! メッセージはそのまま画面のエラーダイアログに表示できる日本語にする。

#![allow(dead_code)] // 各コマンドから使われるまでの暫定(docs/技術設計.md の実装ステップ③以降)

use std::path::PathBuf;

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
}

pub type Result<T> = std::result::Result<T, AppError>;

/// Tauriのコマンドからフロントエンドへ返すため、メッセージ文字列としてシリアライズする。
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
