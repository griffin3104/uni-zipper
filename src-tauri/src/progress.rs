//! 進捗通知とキャンセル(docs/技術設計.md 5.6)。

#![allow(dead_code)] // 各コマンドから使われるまでの暫定(実装ステップ⑤)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

/// キャンセル要求の共有フラグ。複製しても同じフラグを指す。
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// 処理の進み具合。バイト数(非圧縮サイズ)で数える。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}
