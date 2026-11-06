//! 進捗通知とキャンセル(docs/技術設計.md 5.6)。

#![allow(dead_code)] // 各コマンドから使われるまでの暫定(実装ステップ⑤)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

/// 進捗通知の最短間隔(テストでは、通知の回数を安定させるため間隔なしにする)。
#[cfg(not(test))]
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
#[cfg(test)]
const PROGRESS_INTERVAL: Duration = Duration::ZERO;

/// 進捗の集計と通知(間隔を空けて通知する)。
pub struct Tracker<'a> {
    done: u64,
    total: u64,
    last_emit: Instant,
    callback: &'a mut dyn FnMut(Progress),
}

impl<'a> Tracker<'a> {
    pub fn new(total: u64, callback: &'a mut dyn FnMut(Progress)) -> Self {
        Self { done: 0, total, last_emit: Instant::now(), callback }
    }

    /// 今の進み具合を、間隔に関係なく通知する。
    pub fn emit(&mut self) {
        self.last_emit = Instant::now();
        (self.callback)(Progress { done: self.done, total: self.total });
    }

    /// `bytes` 進んだことを記録する。前の通知から間隔が空いていれば通知する。
    pub fn add(&mut self, bytes: u64) {
        self.done += bytes;
        if self.last_emit.elapsed() >= PROGRESS_INTERVAL {
            self.emit();
        }
    }

    /// 完了。全体に達した状態で通知する。
    pub fn finish(&mut self) {
        self.done = self.total;
        self.emit();
    }
}
