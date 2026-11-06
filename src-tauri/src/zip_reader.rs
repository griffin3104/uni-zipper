//! Zipの解凍(docs/技術設計.md 5章)。
//!
//! 流れ: [`inspect`] でドロップ時に中身を調べる(暗号化の有無・文字コードの判定)→
//! [`extract`] で、Zipと同じ場所に自動のフォルダを作って展開する。

#![allow(dead_code)] // Tauriのコマンドから使われるまでの暫定(実装ステップ⑤)

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use zip::HasZipMetadata;
use zip::ZipArchive;
use zip::read::ZipFile;
use zip::result::ZipError;

use crate::encoding::{Charset, Detection, PreviewRow, RawName, decode_name, detect, preview};
use crate::error::{AppError, Result};
use crate::exclude::is_excluded_path;
use crate::normalize::{Platform, UnsafeReason, dedupe_paths, sanitize_entry_path};
use crate::progress::{CancelToken, Progress};
use crate::workdir::numbered_name;

/// プレビューに出す名前の最大件数。
pub const PREVIEW_LIMIT: usize = 5;

/// 進捗通知の最短間隔(テストでは、通知の回数を安定させるため間隔なしにする)。
#[cfg(not(test))]
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
#[cfg(test)]
const PROGRESS_INTERVAL: Duration = Duration::ZERO;

/// 展開時の読み書きの単位(キャンセルの確認もこの単位で行う)。
const CHUNK_SIZE: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// 結果の型
// ---------------------------------------------------------------------------

/// ドロップ時に調べたZipの情報。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZipInspection {
    pub entry_count: usize,
    /// 暗号化されたエントリがある(パスワードが要る)。
    pub encrypted: bool,
    /// ファイル名の文字コードの判定結果。
    pub detection: Detection,
    /// `detection` が曖昧のときだけ入る、2通りの読み方のプレビュー。
    pub preview: Vec<PreviewRow>,
}

/// 展開しなかったエントリ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedEntry {
    /// Zipに入っていたときの名前。
    pub name: String,
    /// 日本語の理由(完了メッセージにそのまま出せる)。
    pub reason: String,
}

/// 使えない文字などを置き換えて展開したエントリ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenamedEntry {
    pub from: String,
    pub to: String,
}

/// 展開の結果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractReport {
    /// 展開先のフォルダ。
    pub out_dir: PathBuf,
    /// 展開したファイルの数(フォルダは数えない)。
    pub file_count: usize,
    pub skipped: Vec<SkippedEntry>,
    pub renamed: Vec<RenamedEntry>,
}

/// 展開の指定。
pub struct ExtractOptions<'a> {
    /// ファイル名の文字コード。`None` なら自動判定し、決まらなければ `CharsetNotResolved`。
    pub charset: Option<Charset>,
    pub password: Option<&'a str>,
    /// 展開先のOS。Windowsだけ、使えない文字を置き換える。
    pub platform: Platform,
    pub cancel: &'a CancelToken,
}

// ---------------------------------------------------------------------------
// Zipの読み込み
// ---------------------------------------------------------------------------

/// セントラルディレクトリから読んだ、エントリ1つ分の情報。
struct EntryMeta {
    index: usize,
    name: Vec<u8>,
    utf8_flag: bool,
    size: u64,
    is_dir: bool,
    is_symlink: bool,
    encrypted: bool,
}

/// 汎用ビットフラグのビット11(ファイル名がUTF-8)。
const UTF8_FLAG: u16 = 0x0800;

fn zip_error(e: ZipError) -> AppError {
    match e {
        ZipError::Io(e) => AppError::Io(e),
        ZipError::InvalidPassword => AppError::WrongPassword,
        other => AppError::InvalidZip(other.to_string()),
    }
}

fn open_archive(path: &Path) -> Result<ZipArchive<BufReader<File>>> {
    let file = File::open(path)?;
    ZipArchive::new(BufReader::new(file)).map_err(zip_error)
}

fn read_entries<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<Vec<EntryMeta>> {
    let mut metas = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index).map_err(zip_error)?;
        metas.push(EntryMeta {
            index,
            name: file.name_raw().to_vec(),
            utf8_flag: file.flags().as_u16() & UTF8_FLAG != 0,
            size: file.size(),
            is_dir: file.is_dir(),
            is_symlink: file.is_symlink(),
            encrypted: file.encrypted(),
        });
    }
    Ok(metas)
}

fn raw_names(metas: &[EntryMeta]) -> Vec<RawName<'_>> {
    metas
        .iter()
        .map(|m| RawName { bytes: &m.name, utf8_flag: m.utf8_flag })
        .collect()
}

/// ドロップ時にZipの中身を調べる。展開はしない。
pub fn inspect(zip_path: &Path) -> Result<ZipInspection> {
    let mut archive = open_archive(zip_path)?;
    let metas = read_entries(&mut archive)?;
    let names = raw_names(&metas);
    let detection = detect(&names);
    let preview = match detection {
        Detection::Ambiguous { .. } => preview(&names, PREVIEW_LIMIT),
        _ => Vec::new(),
    };
    Ok(ZipInspection {
        entry_count: metas.len(),
        encrypted: metas.iter().any(|m| m.encrypted),
        detection,
        preview,
    })
}

// ---------------------------------------------------------------------------
// 展開
// ---------------------------------------------------------------------------

/// 展開する1エントリの計画。
struct Planned {
    index: usize,
    /// 展開先からの相対パス。`/` 区切り。ディレクトリは末尾が `/`。
    path: String,
    original: String,
    replaced: bool,
    size: u64,
    is_dir: bool,
    encrypted: bool,
}

/// Zipを、同じ場所の自動のフォルダ(Zipの拡張子を除いた名前。あれば `_1`、`_2`)に展開する。
///
/// - 途中でキャンセル・エラーになったら、作りかけの展開先フォルダは削除する。
/// - 暗号化されていて `password` が無ければ `PasswordRequired`、違えば `WrongPassword`。
///   どちらも展開先フォルダを作る前(または作った直後のエラー)に返るので、やり直せる。
/// - 読めない・書けないエントリは、スキップして続行し、`ExtractReport::skipped` に載せる。
pub fn extract(
    zip_path: &Path,
    options: &ExtractOptions,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<ExtractReport> {
    let mut archive = open_archive(zip_path)?;
    let metas = read_entries(&mut archive)?;
    let charset = resolve_charset(&metas, options.charset)?;

    let mut skipped = Vec::new();
    let (plan, renamed) = make_plan(&metas, charset, options.platform, &mut skipped);

    check_password(&mut archive, &plan, options.password)?;

    let total: u64 = plan.iter().filter(|p| !p.is_dir).map(|p| p.size).sum();
    let mut tracker = Tracker::new(total, on_progress);
    tracker.emit();

    if options.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let out_dir = create_out_dir(zip_path)?;
    let mut guard = OutDirGuard::new(out_dir.clone());

    let mut file_count = 0;
    for p in &plan {
        if options.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let target = out_dir.join(p.path.trim_end_matches('/'));
        if p.is_dir {
            if let Err(e) = fs::create_dir_all(&target) {
                skipped.push(SkippedEntry {
                    name: p.original.clone(),
                    reason: format!("フォルダを作れませんでした({e})"),
                });
            }
            continue;
        }
        let ctx = FileContext { password: options.password, cancel: options.cancel };
        match extract_file(&mut archive, p, &target, &ctx, &mut tracker) {
            Ok(()) => file_count += 1,
            Err(FileError::Fatal(e)) => return Err(e),
            Err(FileError::Skip(reason)) => {
                let _ = fs::remove_file(&target);
                skipped.push(SkippedEntry { name: p.original.clone(), reason });
            }
        }
    }

    tracker.finish();
    guard.keep();
    Ok(ExtractReport { out_dir, file_count, skipped, renamed })
}

fn resolve_charset(metas: &[EntryMeta], requested: Option<Charset>) -> Result<Charset> {
    if let Some(charset) = requested {
        return Ok(charset);
    }
    match detect(&raw_names(metas)) {
        Detection::NotNeeded => Ok(Charset::Utf8),
        Detection::Resolved { charset } => Ok(charset),
        Detection::Ambiguous { .. } => Err(AppError::CharsetNotResolved),
    }
}

/// 全エントリの名前を決め、展開するもの・スキップするもの・名前を変えるものに振り分ける。
fn make_plan(
    metas: &[EntryMeta],
    charset: Charset,
    platform: Platform,
    skipped: &mut Vec<SkippedEntry>,
) -> (Vec<Planned>, Vec<RenamedEntry>) {
    let mut plan: Vec<Planned> = Vec::new();
    let mut seen_dirs: HashSet<String> = HashSet::new();

    for m in metas {
        let raw = RawName { bytes: &m.name, utf8_flag: m.utf8_flag };
        let original = decode_name(&raw, charset);

        // OS由来の不要ファイルは、黙って除外する(スキップ一覧にも載せない)。
        if is_excluded_path(&original) {
            continue;
        }
        if m.is_symlink {
            skipped.push(SkippedEntry {
                name: original,
                reason: "シンボリックリンクは展開しません".to_string(),
            });
            continue;
        }
        let safe = match sanitize_entry_path(&original, platform) {
            Ok(safe) => safe,
            Err(UnsafeReason::Traversal) => {
                skipped.push(unsafe_entry(original, "展開先の外に出るパスです"));
                continue;
            }
            Err(UnsafeReason::Absolute) => {
                skipped.push(unsafe_entry(original, "絶対パスです"));
                continue;
            }
            // 名前が空・`.` だけのエントリは、危険ではないので黙って無視する。
            Err(UnsafeReason::Empty) => continue,
        };

        let is_dir = m.is_dir || safe.is_dir;
        let path = if is_dir { format!("{}/", safe.path) } else { safe.path };
        // 同じフォルダのエントリが複数あっても、1つにまとめる(連番は付けない)。
        if is_dir && !seen_dirs.insert(path.to_lowercase()) {
            continue;
        }
        plan.push(Planned {
            index: m.index,
            path,
            original,
            replaced: safe.replaced,
            size: m.size,
            is_dir,
            encrypted: m.encrypted,
        });
    }

    // 正規化や置き換えの結果、同名になったものに連番を付ける。
    let before: Vec<String> = plan.iter().map(|p| p.path.clone()).collect();
    let after = dedupe_paths(&before);
    let mut renamed = Vec::new();
    for (p, (old, new)) in plan.iter_mut().zip(before.iter().zip(after)) {
        let changed = &new != old;
        p.path = new;
        if !p.is_dir && (p.replaced || changed) {
            renamed.push(RenamedEntry { from: p.original.clone(), to: p.path.clone() });
        }
    }
    (plan, renamed)
}

fn unsafe_entry(name: String, reason: &str) -> SkippedEntry {
    SkippedEntry { name, reason: reason.to_string() }
}

/// 展開を始める前に、パスワードの有無と正しさを確かめる。
///
/// 暗号化されたファイルが1つでもあるのにパスワードが無ければ `PasswordRequired`。
/// 最初の暗号化ファイルを実際に開いて、パスワードが違えば `WrongPassword`(展開先は作らない)。
fn check_password<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    plan: &[Planned],
    password: Option<&str>,
) -> Result<()> {
    let Some(first) = plan.iter().find(|p| p.encrypted && !p.is_dir) else {
        return Ok(());
    };
    let Some(password) = password else {
        return Err(AppError::PasswordRequired);
    };
    archive
        .by_index_decrypt(first.index, password.as_bytes())
        .map(|_| ())
        .map_err(zip_error)
}

/// Zipと同じ場所に、展開先フォルダを作る。
fn create_out_dir(zip_path: &Path) -> Result<PathBuf> {
    let parent = match zip_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let stem = zip_path
        .file_stem()
        .ok_or_else(|| AppError::InvalidPath(zip_path.to_path_buf()))?
        .to_string_lossy()
        .into_owned();

    for n in 0.. {
        let name = if n == 0 { stem.clone() } else { numbered_name(&stem, n, true) };
        let dir = parent.join(name);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!("連番は尽きない")
}

/// 展開先フォルダの後始末。`keep` を呼ばずに破棄されたら(キャンセル・エラー)、フォルダを削除する。
struct OutDirGuard {
    path: PathBuf,
    keep: bool,
}

impl OutDirGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, keep: false }
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for OutDirGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// 進捗の集計と通知(間隔を空けて通知する)。
struct Tracker<'a> {
    done: u64,
    total: u64,
    last_emit: Instant,
    callback: &'a mut dyn FnMut(Progress),
}

impl<'a> Tracker<'a> {
    fn new(total: u64, callback: &'a mut dyn FnMut(Progress)) -> Self {
        Self { done: 0, total, last_emit: Instant::now(), callback }
    }

    fn emit(&mut self) {
        self.last_emit = Instant::now();
        (self.callback)(Progress { done: self.done, total: self.total });
    }

    fn add(&mut self, bytes: u64) {
        self.done += bytes;
        if self.last_emit.elapsed() >= PROGRESS_INTERVAL {
            self.emit();
        }
    }

    fn finish(&mut self) {
        self.done = self.total;
        self.emit();
    }
}

struct FileContext<'a> {
    password: Option<&'a str>,
    cancel: &'a CancelToken,
}

/// 1ファイルの展開で起きた失敗。
enum FileError {
    /// 全体を止める(キャンセル、パスワード違い、ディスクへの書き込み失敗)。
    Fatal(AppError),
    /// このファイルだけスキップして続ける。
    Skip(String),
}

fn extract_file<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    p: &Planned,
    target: &Path,
    ctx: &FileContext,
    tracker: &mut Tracker,
) -> std::result::Result<(), FileError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| FileError::Skip(format!("フォルダを作れませんでした({e})")))?;
    }

    let mut entry = open_entry(archive, p, ctx.password).map_err(|e| match e {
        ZipError::InvalidPassword => FileError::Fatal(AppError::WrongPassword),
        ZipError::Io(e) => FileError::Skip(format!("読み込めませんでした({e})")),
        other => FileError::Skip(format!("読み込めませんでした({other})")),
    })?;
    let is_aes = entry.get_metadata().aes_settings().is_some();

    let mut out = File::create(target)
        .map_err(|e| FileError::Skip(format!("書き込めませんでした({e})")))?;

    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        if ctx.cancel.is_cancelled() {
            return Err(FileError::Fatal(AppError::Cancelled));
        }
        let n = match entry.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Err(read_failure(&e, p.encrypted && !is_aes)),
        };
        out.write_all(&buf[..n]).map_err(|e| FileError::Fatal(AppError::Io(e)))?;
        tracker.add(n as u64);
    }
    out.flush().map_err(|e| FileError::Fatal(AppError::Io(e)))?;
    Ok(())
}

fn open_entry<'a, R: Read + Seek>(
    archive: &'a mut ZipArchive<R>,
    p: &Planned,
    password: Option<&str>,
) -> std::result::Result<ZipFile<'a, R>, ZipError> {
    if p.encrypted {
        archive.by_index_decrypt(p.index, password.unwrap_or("").as_bytes())
    } else {
        archive.by_index(p.index)
    }
}

/// 読み込み中のエラーを分類する。
///
/// ZipCryptoは、誤ったパスワードでも先頭の検証を通ることがある(約1/256)。その場合、
/// 復号結果が壊れて、展開後のCRCが合わない(または解凍に失敗する)ので、「パスワード違い」とみなす。
/// AESは、開く時点の検証で誤りを確実に検出できるので、読み込み中の失敗は壊れたデータとして扱う。
fn read_failure(e: &std::io::Error, zipcrypto: bool) -> FileError {
    if zipcrypto {
        FileError::Fatal(AppError::WrongPassword)
    } else {
        FileError::Skip(format!("Zipの中身が壊れています({e})"))
    }
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::encode_shift_jis_strict;
    use crate::test_util::{
        Enc, build_zip, build_zip_stored, build_zip_with_symlink, clear_utf8_flag,
    };
    use tempfile::TempDir;
    use unicode_normalization::UnicodeNormalization;

    /// 一時フォルダにZipを書く。
    fn write_zip(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn sjis(s: &str) -> Vec<u8> {
        encode_shift_jis_strict(s).unwrap()
    }

    fn run(zip: &Path, charset: Option<Charset>, password: Option<&str>, platform: Platform) -> Result<ExtractReport> {
        let cancel = CancelToken::new();
        let options = ExtractOptions { charset, password, platform, cancel: &cancel };
        extract(zip, &options, &mut |_| {})
    }

    fn run_default(zip: &Path) -> Result<ExtractReport> {
        run(zip, None, None, Platform::Other)
    }

    fn read(path: impl AsRef<Path>) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn utf8フラグ付きの日本語名を展開できる() {
        let dir = TempDir::new().unwrap();
        let zip = write_zip(
            &dir,
            "test.zip",
            &build_zip(
                &[
                    ("テスト.txt".as_bytes(), b"a"),
                    ("議事録/".as_bytes(), b""),
                    ("議事録/第1回.txt".as_bytes(), b"b"),
                ],
                Enc::None,
            ),
        );
        let report = run_default(&zip).unwrap();
        assert_eq!(report.out_dir, dir.path().join("test"));
        assert_eq!(report.file_count, 2);
        assert!(report.skipped.is_empty() && report.renamed.is_empty());
        assert_eq!(read(report.out_dir.join("テスト.txt")), "a");
        assert_eq!(read(report.out_dir.join("議事録/第1回.txt")), "b");
    }

    #[test]
    fn フラグなしのutf8とnfdの濁点は_nfcの名前で展開される() {
        // Macのアーカイブユーティリティ相当: UTF-8だがフラグなし、濁点は分解形。
        let nfd: String = "がぎぐ.txt".nfd().collect();
        let mut bytes = build_zip(&[(nfd.as_bytes(), b"x")], Enc::None);
        clear_utf8_flag(&mut bytes);
        let dir = TempDir::new().unwrap();
        let zip = write_zip(&dir, "mac.zip", &bytes);

        let info = inspect(&zip).unwrap();
        assert_eq!(info.detection, Detection::Resolved { charset: Charset::Utf8 });

        let report = run_default(&zip).unwrap();
        let names: Vec<String> = fs::read_dir(&report.out_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["がぎぐ.txt"]);
        assert_eq!(names[0], names[0].nfc().collect::<String>());
        assert!(report.renamed.is_empty(), "NFC正規化だけなら通知しない");
    }

    #[test]
    fn shift_jisの名前を展開できる() {
        let dir = TempDir::new().unwrap();
        let (a, b) = (sjis("議事録/"), sjis("議事録/第1回〜第3回.txt".replace('〜', "～").as_str()));
        let zip = write_zip(&dir, "win.zip", &build_zip(&[(&a, b""), (&b, b"s")], Enc::None));

        let info = inspect(&zip).unwrap();
        assert_eq!(info.detection, Detection::Resolved { charset: Charset::ShiftJis });
        assert!(info.preview.is_empty());

        let report = run_default(&zip).unwrap();
        assert_eq!(read(report.out_dir.join("議事録/第1回～第3回.txt")), "s");
    }

    #[test]
    fn 文字コードが曖昧なら指定が要る() {
        // 「ア.txt」のUTF-8は、Shift_JISとしても読めて、日本語が1文字だけなので決められない。
        let dir = TempDir::new().unwrap();
        let mut bytes = build_zip(&[("ア.txt".as_bytes(), b"1")], Enc::None);
        clear_utf8_flag(&mut bytes);
        let zip = write_zip(&dir, "amb.zip", &bytes);

        let info = inspect(&zip).unwrap();
        assert!(matches!(info.detection, Detection::Ambiguous { .. }));
        assert_eq!(info.preview.len(), 1);

        assert!(matches!(run_default(&zip), Err(AppError::CharsetNotResolved)));
        assert!(!dir.path().join("amb").exists(), "失敗時は展開先を作らない");

        let report = run(&zip, Some(Charset::Utf8), None, Platform::Other).unwrap();
        assert_eq!(read(report.out_dir.join("ア.txt")), "1");
    }

    #[test]
    fn 暗号化の有無を調べられる() {
        let dir = TempDir::new().unwrap();
        let plain = write_zip(&dir, "plain.zip", &build_zip(&[(b"a.txt", b"1")], Enc::None));
        let zc = write_zip(&dir, "zc.zip", &build_zip(&[(b"a.txt", b"1")], Enc::ZipCrypto("pw")));
        let aes = write_zip(&dir, "aes.zip", &build_zip(&[(b"a.txt", b"1")], Enc::Aes256("pw")));
        assert!(!inspect(&plain).unwrap().encrypted);
        assert!(inspect(&zc).unwrap().encrypted);
        assert!(inspect(&aes).unwrap().encrypted);
    }

    #[test]
    fn パスワード付きzipを展開できる() {
        for (label, enc) in [("zipcrypto", Enc::ZipCrypto("秘密pw")), ("aes", Enc::Aes256("秘密pw"))] {
            let dir = TempDir::new().unwrap();
            let zip = write_zip(
                &dir,
                "locked.zip",
                &build_zip(&[("機密.txt".as_bytes(), "中身".as_bytes())], enc),
            );
            let report = run(&zip, None, Some("秘密pw"), Platform::Other).unwrap();
            assert_eq!(read(report.out_dir.join("機密.txt")), "中身", "{label}");
        }
    }

    #[test]
    fn パスワードが無い_違うときはエラーで展開先を作らない() {
        for (label, enc) in [("zipcrypto", Enc::ZipCrypto("right")), ("aes", Enc::Aes256("right"))] {
            let dir = TempDir::new().unwrap();
            let data = vec![b'x'; 10_000];
            let zip = write_zip(&dir, "locked.zip", &build_zip(&[(b"a.txt", &data)], enc));

            let none = run(&zip, None, None, Platform::Other);
            assert!(matches!(none, Err(AppError::PasswordRequired)), "{label}");
            let wrong = run(&zip, None, Some("wrong"), Platform::Other);
            assert!(matches!(wrong, Err(AppError::WrongPassword)), "{label}");
            assert!(!dir.path().join("locked").exists(), "{label}");
        }
    }

    #[test]
    fn 危険なパスはスキップ一覧に載せ_展開先の外に書かない() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("sandbox");
        fs::create_dir(&out).unwrap();
        let zip_path = out.join("evil.zip");
        let bytes = build_zip(
            &[
                (b"../evil.txt", b"x"),
                (b"a/../../evil2.txt", b"x"),
                (b"/abs.txt", b"x"),
                (b"C:\\win.txt", b"x"),
                (b"good.txt", b"ok"),
            ],
            Enc::None,
        );
        fs::write(&zip_path, bytes).unwrap();

        let report = run_default(&zip_path).unwrap();
        assert_eq!(report.file_count, 1);
        assert_eq!(report.skipped.len(), 4);
        assert_eq!(read(report.out_dir.join("good.txt")), "ok");
        assert!(!out.join("evil.txt").exists() && !dir.path().join("evil.txt").exists());
        assert!(!out.join("evil2.txt").exists() && !dir.path().join("evil2.txt").exists());
        assert!(!out.join("abs.txt").exists());
    }

    #[test]
    fn os由来の不要ファイルは黙って除外する() {
        let dir = TempDir::new().unwrap();
        let zip = write_zip(
            &dir,
            "mac.zip",
            &build_zip(
                &[
                    (b".DS_Store", b"x"),
                    (b"__MACOSX/", b""),
                    (b"__MACOSX/._a.txt", b"x"),
                    (b"d/Thumbs.db", b"x"),
                    (b"d/desktop.ini", b"x"),
                    (b"d/a.txt", b"keep"),
                ],
                Enc::None,
            ),
        );
        let report = run_default(&zip).unwrap();
        assert_eq!(report.file_count, 1);
        assert!(report.skipped.is_empty());
        assert!(!report.out_dir.join("__MACOSX").exists());
        assert!(!report.out_dir.join(".DS_Store").exists());
        assert!(!report.out_dir.join("d/Thumbs.db").exists());
        assert_eq!(read(report.out_dir.join("d/a.txt")), "keep");
    }

    #[test]
    fn シンボリックリンクは展開せずスキップ一覧に載せる() {
        let dir = TempDir::new().unwrap();
        let zip = write_zip(&dir, "link.zip", &build_zip_with_symlink("link", "/etc/passwd"));
        let report = run_default(&zip).unwrap();
        assert_eq!(report.file_count, 1);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].name, "link");
        assert!(fs::symlink_metadata(report.out_dir.join("link")).is_err());
    }

    #[test]
    fn 展開先が既にあれば連番を付ける() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("a")).unwrap();
        fs::create_dir(dir.path().join("a_1")).unwrap();
        let zip = write_zip(&dir, "a.zip", &build_zip(&[(b"f.txt", b"1")], Enc::None));
        let report = run_default(&zip).unwrap();
        assert_eq!(report.out_dir, dir.path().join("a_2"));
        assert!(dir.path().join("a").read_dir().unwrap().next().is_none(), "既存のフォルダは触らない");
    }

    #[test]
    fn 名前が同じになるエントリには連番を付けて_一覧に載せる() {
        let dir = TempDir::new().unwrap();
        let zip = write_zip(
            &dir,
            "dup.zip",
            &build_zip(&[(b"A.txt", b"upper"), (b"a.txt", b"lower")], Enc::None),
        );
        let report = run_default(&zip).unwrap();
        assert_eq!(report.file_count, 2);
        assert_eq!(read(report.out_dir.join("A.txt")), "upper");
        assert_eq!(read(report.out_dir.join("a_1.txt")), "lower");
        assert_eq!(report.renamed, [RenamedEntry { from: "a.txt".into(), to: "a_1.txt".into() }]);
    }

    #[test]
    fn windowsでは使えない文字を置き換えて一覧に載せる() {
        let dir = TempDir::new().unwrap();
        let zip = write_zip(
            &dir,
            "w.zip",
            &build_zip(&[(b"a:b.txt", b"1"), (b"CON.txt", b"2"), (b"ok.txt", b"3")], Enc::None),
        );
        let report = run(&zip, None, None, Platform::Windows).unwrap();
        assert_eq!(read(report.out_dir.join("a_b.txt")), "1");
        assert_eq!(read(report.out_dir.join("CON_.txt")), "2");
        assert_eq!(
            report.renamed,
            [
                RenamedEntry { from: "a:b.txt".into(), to: "a_b.txt".into() },
                RenamedEntry { from: "CON.txt".into(), to: "CON_.txt".into() },
            ]
        );
        // macOSでは、そのまま展開する(`:` はFinder上で `/` に見える)
        let dir2 = TempDir::new().unwrap();
        let zip2 = write_zip(&dir2, "w.zip", &build_zip(&[(b"a:b.txt", b"1")], Enc::None));
        let report2 = run(&zip2, None, None, Platform::Other).unwrap();
        assert!(report2.renamed.is_empty());
    }

    #[test]
    fn バックスラッシュ区切りのzipも展開できる() {
        let dir = TempDir::new().unwrap();
        let zip = write_zip(&dir, "old.zip", &build_zip(&[(b"d\\e\\f.txt", b"1")], Enc::None));
        let report = run_default(&zip).unwrap();
        assert_eq!(read(report.out_dir.join("d/e/f.txt")), "1");
    }

    #[test]
    fn キャンセルすると展開先を削除する() {
        let dir = TempDir::new().unwrap();
        let zip = write_zip(&dir, "c.zip", &build_zip(&[(b"a.txt", b"1")], Enc::None));
        let cancel = CancelToken::new();
        cancel.cancel();
        let options = ExtractOptions {
            charset: None,
            password: None,
            platform: Platform::Other,
            cancel: &cancel,
        };
        let result = extract(&zip, &options, &mut |_| {});
        assert!(matches!(result, Err(AppError::Cancelled)));
        assert!(!dir.path().join("c").exists());
    }

    #[test]
    fn 途中でキャンセルすると展開済みの分も削除する() {
        let dir = TempDir::new().unwrap();
        let big = vec![b'z'; 1024 * 1024];
        let zip = write_zip(
            &dir,
            "m.zip",
            &build_zip(&[(b"1.bin", &big), (b"2.bin", &big), (b"3.bin", &big)], Enc::None),
        );
        let cancel = CancelToken::new();
        let options = ExtractOptions {
            charset: None,
            password: None,
            platform: Platform::Other,
            cancel: &cancel,
        };
        // 展開が始まって、1バイトでも進んだ時点でキャンセルを立てる。
        let result = extract(&zip, &options, &mut |p| {
            if p.done > 0 {
                cancel.cancel();
            }
        });
        assert!(matches!(result, Err(AppError::Cancelled)));
        assert!(!dir.path().join("m").exists(), "展開済みの分も含めて削除される");
    }

    #[test]
    fn 進捗は最後に全体に達する() {
        let dir = TempDir::new().unwrap();
        let data = vec![b'q'; 300_000];
        let zip = write_zip(&dir, "p.zip", &build_zip(&[(b"a.bin", &data), (b"b.bin", &data)], Enc::None));
        let cancel = CancelToken::new();
        let options = ExtractOptions {
            charset: None,
            password: None,
            platform: Platform::Other,
            cancel: &cancel,
        };
        let mut seen: Vec<Progress> = Vec::new();
        extract(&zip, &options, &mut |p| seen.push(p)).unwrap();
        assert_eq!(seen.first(), Some(&Progress { done: 0, total: 600_000 }));
        assert_eq!(seen.last(), Some(&Progress { done: 600_000, total: 600_000 }));
        assert!(seen.windows(2).all(|w| w[0].done <= w[1].done));
    }

    #[test]
    fn 壊れたファイルはスキップして他は展開する() {
        let marker = b"CORRUPTME-CORRUPTME";
        let mut bytes = build_zip_stored(&[(b"bad.txt", marker), (b"good.txt", b"fine")]);
        let at = bytes.windows(marker.len()).position(|w| w == marker).unwrap();
        bytes[at] ^= 0xFF; // 中身を1バイト壊す → CRC不一致
        let dir = TempDir::new().unwrap();
        let zip = write_zip(&dir, "bad.zip", &bytes);

        let report = run_default(&zip).unwrap();
        assert_eq!(report.file_count, 1);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].name, "bad.txt");
        assert!(!report.out_dir.join("bad.txt").exists(), "壊れた途中のファイルは残さない");
        assert_eq!(read(report.out_dir.join("good.txt")), "fine");
    }

    #[test]
    fn zipではないファイルはエラーになる() {
        let dir = TempDir::new().unwrap();
        let path = write_zip(&dir, "no.zip", b"this is not a zip file at all");
        assert!(matches!(inspect(&path), Err(AppError::InvalidZip(_))));
        assert!(matches!(run_default(&path), Err(AppError::InvalidZip(_))));
        assert!(!dir.path().join("no").exists());
    }

    #[test]
    fn windows標準機能で作った実物のzipを展開できる() {
        // tests/fixtures/Folder.zip: 新しいWindowsの標準機能で作ったZip(中身の詳細は同フォルダのREADME)。
        // 日本語名はUTF-8フラグ付き。フォルダのエントリ、データ記述子(ビット3)付きのファイルを含む。
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Folder.zip");
        let dir = TempDir::new().unwrap();
        let zip = dir.path().join("Folder.zip");
        fs::copy(fixture, &zip).unwrap();

        let info = inspect(&zip).unwrap();
        assert_eq!(info.entry_count, 9);
        assert_eq!(info.detection, Detection::NotNeeded, "全エントリがUTF-8フラグ付きなので判定不要");
        assert!(!info.encrypted);

        let report = run_default(&zip).unwrap();
        assert_eq!(report.out_dir, dir.path().join("Folder"));
        assert_eq!(report.file_count, 7);
        assert!(report.skipped.is_empty() && report.renamed.is_empty());
        let base = report.out_dir.join("Folder");
        assert_eq!(read(base.join("がぎぐ.txt")), "がぎぐ");
        assert_eq!(read(base.join("テスト.txt")), "テスト.txt");
        assert_eq!(read(base.join("第1回～第3回.txt")), "議事録です\r\n第1回～第3回");
        assert_eq!(read(base.join("議事録/第1回.txt")), "議事録です\r\n議事録/第1回");
    }

    /// fixtures のZip(のバイト列)を一時フォルダに書いて、検査・展開し、展開先の名前一覧(ソート済み)を返す。
    fn extract_bytes(name: &str, bytes: &[u8]) -> (TempDir, ZipInspection, ExtractReport, Vec<String>) {
        let dir = TempDir::new().unwrap();
        let zip = write_zip(&dir, name, bytes);
        let info = inspect(&zip).unwrap();
        let report = run_default(&zip).unwrap();
        let stem = name.trim_end_matches(".zip");
        let mut names: Vec<String> = fs::read_dir(report.out_dir.join(stem))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        (dir, info, report, names)
    }

    fn fixture_bytes(name: &str) -> Vec<u8> {
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
    }

    /// 両方のZipに共通する、元のファイル名。
    fn expected_names() -> Vec<String> {
        let mut names: Vec<String> = [
            "①.txt", "がぎぐ.txt", "ぱぴぷ.txt", "テスト.txt", "㈱.txt", "日本語.txt",
            "第1回〜第3回（チ）.txt", "第1回～第3回（波）.txt", "資料.txt", "ｱｲｳ.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        names.sort();
        names
    }

    #[test]
    fn windows11のエクスプローラーで作ったzipを展開できる() {
        // 全エントリがUTF-8フラグ付き(波ダッシュ〜も、半角カナも、①㈱もそのまま)。
        let (_dir, info, report, names) = extract_bytes("win11_explorer.zip", &fixture_bytes("win11_explorer.zip"));
        assert_eq!(info.detection, Detection::NotNeeded);
        assert_eq!(report.file_count, 10);
        assert!(report.skipped.is_empty() && report.renamed.is_empty());
        assert_eq!(names, expected_names());
    }

    #[test]
    fn sevenzipが作ったzipは_unicodeパス拡張フィールドで正しい名前になる() {
        // 7-Zipは、Shift_JISの名前に加えて、UTF-8の名前を拡張フィールド(0x7075)にも持たせる。
        // `zip` クレートがこれを解釈するので、そのままで名前が正しく読める(判定も不要になる)。
        let (_dir, info, report, names) = extract_bytes("win11_7zip.zip", &fixture_bytes("win11_7zip.zip"));
        assert_eq!(info.detection, Detection::NotNeeded);
        assert_eq!(report.file_count, 10);
        assert!(report.skipped.is_empty() && report.renamed.is_empty());
        assert_eq!(names, expected_names());
    }

    #[test]
    fn sevenzipのzipから拡張フィールドを除いても_shift_jisとして展開できる() {
        // 拡張フィールドが無いZip(他のツールや、拡張フィールドを付けない設定)を再現する。
        // Shift_JISに変換できない名前(〜)だけUTF-8フラグ付きで、他はShift_JIS(フラグなし)が混在する。
        // フラグ付きは判定から外れ、残りがShift_JISで確定する。
        let mut bytes = fixture_bytes("win11_7zip.zip");
        crate::test_util::disable_unicode_path_field(&mut bytes);
        let (_dir, info, report, names) = extract_bytes("win11_7zip.zip", &bytes);
        assert_eq!(info.detection, Detection::Resolved { charset: Charset::ShiftJis });
        assert!(info.preview.is_empty());
        assert_eq!(report.file_count, 10);
        assert!(report.skipped.is_empty() && report.renamed.is_empty());
        assert_eq!(names, expected_names());
    }
}
