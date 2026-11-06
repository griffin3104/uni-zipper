//! Zipの圧縮(docs/技術設計.md 6章)。
//!
//! ワークフォルダ(ドロップされたものをコピーしたフラットな置き場)の中身を、1つのZipにする。
//! 書き込みは、保存先と同じ場所の一時ファイルに行い、最後に保存先へ移す
//! (キャンセル・エラーで、既存のZipを壊さない)。


use std::collections::HashSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use zip::unstable::write::FileOptionsExt;
use zip::write::{FileOptions, SimpleFileOptions};
use zip::{AesMode, CompressionMethod, DateTime, System, ZipWriter};

use crate::error::{AppError, Result};
use crate::exclude::is_excluded_name;
use crate::normalize::{
    NameEncoding, dedupe_paths, encode_windows_name, to_nfc, windows_text,
};
use crate::progress::{CancelToken, Progress, Tracker};
use crate::zip_reader::{RenamedEntry, SkippedEntry, zip_error};

/// 書き込みの単位(キャンセルの確認もこの単位で行う)。
const CHUNK_SIZE: usize = 64 * 1024;

/// この大きさ以上のファイルは、Zip64形式で書く(通常のZipは4GiB未満)。
const ZIP64_THRESHOLD: u64 = u32::MAX as u64;

/// 圧縮先のOS(画面の「自動」「Windows」)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetOs {
    /// すべてUTF-8(UTF-8フラグあり)、NFC。
    Auto,
    /// Shift_JIS優先(7-Zip方式)。変換できない名前だけUTF-8フラグ付き。
    Windows,
}

/// 暗号化の方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EncryptionMethod {
    ZipCrypto,
    Aes256,
}

/// 圧縮の指定。
pub struct CompressOptions<'a> {
    pub target: TargetOs,
    /// 方式とパスワード。パスワードが空なら、暗号化しない。
    pub encryption: Option<(EncryptionMethod, &'a str)>,
    pub cancel: &'a CancelToken,
}

/// 圧縮の結果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressReport {
    pub zip_path: PathBuf,
    /// 書き込んだファイルの数(フォルダは数えない)。
    pub file_count: usize,
    pub skipped: Vec<SkippedEntry>,
    /// 名前を変えたもの(`〜` の置き換え、同名の連番)。
    pub renamed: Vec<RenamedEntry>,
}

// ---------------------------------------------------------------------------
// 元ファイルの一覧化
// ---------------------------------------------------------------------------

/// ワークフォルダ内の1エントリ(ファイルまたはフォルダ)。
struct SrcEntry {
    /// ワークフォルダからの相対パス。`/` 区切り、フォルダは末尾が `/`。
    rel: String,
    fs_path: PathBuf,
    meta: Metadata,
    is_dir: bool,
}

/// ワークフォルダの中を、名前順に一覧化する。除外ファイル(`.DS_Store` など)は、フォルダごと含めない。
fn collect_entries(root: &Path, skipped: &mut Vec<SkippedEntry>) -> Result<Vec<SrcEntry>> {
    let walker = WalkDir::new(root)
        .follow_links(false)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| !is_excluded_name(&e.file_name().to_string_lossy()));

    let mut entries = Vec::new();
    for item in walker {
        let entry = match item {
            Ok(entry) => entry,
            Err(e) => {
                let name = e
                    .path()
                    .map(|p| display_rel(root, p))
                    .unwrap_or_default();
                skipped.push(SkippedEntry { name, reason: format!("読み込めませんでした({e})") });
                continue;
            }
        };
        let rel = display_rel(root, entry.path());
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            skipped.push(SkippedEntry {
                name: rel,
                reason: "シンボリックリンクは追加しません".to_string(),
            });
            continue;
        }
        if !file_type.is_dir() && !file_type.is_file() {
            skipped.push(SkippedEntry { name: rel, reason: "通常のファイルではありません".to_string() });
            continue;
        }
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(e) => {
                skipped.push(SkippedEntry { name: rel, reason: format!("読み込めませんでした({e})") });
                continue;
            }
        };
        let is_dir = file_type.is_dir();
        entries.push(SrcEntry {
            rel: if is_dir { format!("{rel}/") } else { rel },
            fs_path: entry.path().to_path_buf(),
            meta,
            is_dir,
        });
    }
    Ok(entries)
}

/// ワークフォルダからの相対パスを、`/` 区切りの文字列にする。
fn display_rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

// ---------------------------------------------------------------------------
// 名前の決定
// ---------------------------------------------------------------------------

/// 書き込む1エントリの計画。
struct Planned {
    src: SrcEntry,
    bytes: Vec<u8>,
    encoding: NameEncoding,
}

/// 全エントリの名前を決める。同名になったものには連番を付ける(フォルダは1つにまとめる)。
fn make_plan(
    entries: Vec<SrcEntry>,
    target: TargetOs,
) -> (Vec<Planned>, Vec<RenamedEntry>) {
    // 1. 文字としての名前(NFC。Windowsは `〜` → `～` も)。
    let mut staged: Vec<(SrcEntry, String, bool)> = Vec::with_capacity(entries.len());
    let mut seen_dirs: HashSet<String> = HashSet::new();
    for src in entries {
        let (text, replaced) = match target {
            TargetOs::Auto => (to_nfc(&src.rel), false),
            TargetOs::Windows => windows_text(&src.rel),
        };
        // 同じフォルダのエントリが複数あっても、1つにまとめる(連番は付けない)。
        if src.is_dir && !seen_dirs.insert(text.to_lowercase()) {
            continue;
        }
        staged.push((src, text, replaced));
    }

    // 2. 同名になったものに連番を付ける。
    let before: Vec<String> = staged.iter().map(|(_, text, _)| text.clone()).collect();
    let after = dedupe_paths(&before);

    // 3. 書き方を決める。
    let mut plan = Vec::with_capacity(staged.len());
    let mut renamed = Vec::new();
    for ((src, _, replaced), (old, new)) in staged.into_iter().zip(before.iter().zip(after)) {
        let changed = &new != old;
        if !src.is_dir && (replaced || changed) {
            renamed.push(RenamedEntry {
                from: src.rel.clone(),
                to: new.clone(),
            });
        }
        let (bytes, encoding) = match target {
            TargetOs::Auto => (new.into_bytes(), NameEncoding::Utf8),
            TargetOs::Windows => encode_windows_name(&new),
        };
        plan.push(Planned { src, bytes, encoding });
    }
    (plan, renamed)
}

// ---------------------------------------------------------------------------
// 圧縮
// ---------------------------------------------------------------------------

/// ワークフォルダ `src_root` の中身を、`dest_zip` にZipとして書く。
///
/// - 保存先と同じ場所の一時ファイルに書いて、最後に保存先へ移す。キャンセル・エラーのときは、
///   一時ファイルを削除し、既存の保存先(あれば)には触らない。保存先に同名のファイルがあれば、置き換える
///   (上書きの確認は、呼び出す側で済ませておく)。
/// - 読み込めないファイルは、スキップして続行し、`CompressReport::skipped` に載せる。
///   書き込み中に読み込みが失敗したときは、全体をエラーにする。
pub fn compress(
    src_root: &Path,
    dest_zip: &Path,
    options: &CompressOptions,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<CompressReport> {
    let mut skipped = Vec::new();
    let entries = collect_entries(src_root, &mut skipped)?;
    let (plan, renamed) = make_plan(entries, options.target);

    let total: u64 = plan.iter().filter(|p| !p.src.is_dir).map(|p| p.src.meta.len()).sum();
    let mut tracker = Tracker::new(total, on_progress);
    tracker.emit();

    if options.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }

    let tmp_path = temp_path(dest_zip)?;
    let mut guard = TempFileGuard::new(tmp_path.clone());
    // 後処理(UTF-8フラグの書き換え)で読み戻すので、読み書き両用で開く。
    let tmp_file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&tmp_path)?;
    let mut writer = ZipWriter::new(tmp_file);

    let encryption = options.encryption.filter(|(_, password)| !password.is_empty());
    let mut file_count = 0;
    // 書き込んだエントリのうち、Shift_JISで書いたもの(UTF-8フラグを落とす対象)。書き込み順。
    let mut clear_flags: Vec<bool> = Vec::new();

    for p in &plan {
        if options.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let opts = entry_options(&p.src, options.target, encryption).map_err(zip_error)?.name(&p.bytes);

        if p.src.is_dir {
            writer.start_file_with_options(opts).map_err(zip_error)?;
            clear_flags.push(p.encoding == NameEncoding::ShiftJis);
            continue;
        }

        // 先に元ファイルを開く(開けなければ、Zipには何も書かずにスキップできる)。
        let mut source = match File::open(&p.src.fs_path) {
            Ok(f) => f,
            Err(e) => {
                skipped.push(SkippedEntry {
                    name: p.src.rel.clone(),
                    reason: format!("読み込めませんでした({e})"),
                });
                continue;
            }
        };
        writer.start_file_with_options(opts).map_err(zip_error)?;
        clear_flags.push(p.encoding == NameEncoding::ShiftJis);

        let mut buf = vec![0u8; CHUNK_SIZE];
        loop {
            if options.cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            let n = source.read(&mut buf)?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n])?;
            tracker.add(n as u64);
        }
        file_count += 1;
    }

    let mut file = writer.finish().map_err(zip_error)?;
    clear_utf8_flags(&mut file, &clear_flags)?;
    file.sync_all()?;
    drop(file);

    fs::rename(&tmp_path, dest_zip)?;
    guard.keep();
    tracker.finish();
    Ok(CompressReport { zip_path: dest_zip.to_path_buf(), file_count, skipped, renamed })
}

/// 1エントリの書き込みオプション(圧縮方法・更新日時・権限・暗号化・Zip64)。
/// 暗号化はファイルだけにかける(フォルダのエントリは中身が無いので、かけない)。
///
/// 「Windows」モードでは、作成元OS(version made by)を、7-Zipと同じ**FAT(MS-DOS)**にする。
/// 7-ZipなどのWindowsのツールは、作成元がUnixのエントリの、UTF-8フラグなしの名前を、
/// UTF-8として読もうとする(Unixのツールは、UTF-8で書くのが普通なため)。
/// Shift_JISの名前が、これで文字化けしてしまうので、FATにして、OSの文字コード(日本語ならShift_JIS)で読ませる。
fn entry_options<'k>(
    src: &SrcEntry,
    target: TargetOs,
    encryption: Option<(EncryptionMethod, &'k str)>,
) -> zip::result::ZipResult<FileOptions<'k, 'static, ()>> {
    let mut opts: FileOptions<'k, 'static, ()> = SimpleFileOptions::default()
        .compression_method(if src.is_dir { CompressionMethod::Stored } else { CompressionMethod::Deflated })
        .large_file(src.meta.len() >= ZIP64_THRESHOLD);
    if let Some(time) = src.meta.modified().ok().and_then(zip_time) {
        opts = opts.last_modified_time(time);
    }
    match target {
        // FATには、Unixの権限が無い。ファイル属性(フォルダ属性など)だけを書く。
        TargetOs::Windows => opts = opts.system(System::Dos),
        // 作成元OSは、どのOSで圧縮しても同じになるよう、Unixに固定する(`zip` クレートの既定は、
        // 動いているOS: Windowsだと、FATになる)。Windows 11標準のZipも、Unix + UTF-8フラグ。
        TargetOs::Auto => {
            opts = opts.system(System::Unix);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                opts = opts.unix_permissions(src.meta.permissions().mode() & 0o777);
            }
            // Windowsには、Unixの権限が無いので、ふつうの値を書く(フォルダが「ファイル」の権限にならないように)。
            #[cfg(not(unix))]
            {
                opts = opts.unix_permissions(if src.is_dir { 0o755 } else { 0o644 });
            }
        }
    }
    if !src.is_dir {
        opts = match encryption {
            None => opts,
            Some((EncryptionMethod::ZipCrypto, password)) => {
                opts.with_deprecated_encryption(password.as_bytes())?
            }
            Some((EncryptionMethod::Aes256, password)) => {
                opts.with_aes_encryption(AesMode::Aes256, password)
            }
        };
    }
    Ok(opts)
}

/// 更新日時を、Zipの日時(ローカル時刻)にする。表せない日時(1980年より前など)は `None`。
fn zip_time(time: std::time::SystemTime) -> Option<DateTime> {
    let local: chrono::DateTime<Local> = time.into();
    DateTime::from_date_and_time(
        u16::try_from(local.year()).ok()?,
        local.month() as u8,
        local.day() as u8,
        local.hour() as u8,
        local.minute() as u8,
        local.second() as u8,
    )
    .ok()
}

/// 保存先と同じ場所の、一時ファイルのパス。
fn temp_path(dest_zip: &Path) -> Result<PathBuf> {
    let name = dest_zip
        .file_name()
        .ok_or_else(|| AppError::InvalidPath(dest_zip.to_path_buf()))?
        .to_string_lossy();
    Ok(dest_zip.with_file_name(format!("{name}.uni-zipper-tmp")))
}

/// 一時ファイルの後始末。`keep` を呼ばずに破棄されたら(キャンセル・エラー)、削除する。
struct TempFileGuard {
    path: PathBuf,
    keep: bool,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, keep: false }
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

// ---------------------------------------------------------------------------
// UTF-8フラグの後処理
// ---------------------------------------------------------------------------

/// `zip` クレートは、名前が「UTF-8として正しく、非ASCII」なら、UTF-8フラグを自動で立てる。
/// Shift_JISのバイト列が偶然UTF-8として正しいと(例: 半角カナ `ﾃｱ` = `C3 B1`)、誤ってフラグが立つ
/// (docs/技術設計.md 6.2)。書き込み完了後に、Shift_JISで書いたエントリだけ、
/// ローカルヘッダとセントラルヘッダの両方で、フラグ(汎用ビットフラグのビット11)を落とす。
///
/// `clear[i]` が `true` のエントリ(セントラルディレクトリの `i` 番目 = 書き込み順)が対象。
fn clear_utf8_flags<S: Read + Write + Seek>(stream: &mut S, clear: &[bool]) -> io::Result<()> {
    if !clear.iter().any(|&c| c) {
        return Ok(());
    }
    let (count, mut pos) = central_directory_location(stream)?;
    if count != clear.len() as u64 {
        return Err(invalid_data("セントラルディレクトリのエントリ数が合いません"));
    }

    for &target in clear {
        let mut header = [0u8; 46];
        read_at(stream, pos, &mut header)?;
        if u32_at(&header, 0) != 0x0201_4b50 {
            return Err(invalid_data("セントラルヘッダのシグネチャが不正です"));
        }
        let name_len = u64::from(u16_at(&header, 28));
        let extra_len = u64::from(u16_at(&header, 30));
        let comment_len = u64::from(u16_at(&header, 32));

        if target {
            let mut extra = vec![0u8; extra_len as usize];
            read_at(stream, pos + 46 + name_len, &mut extra)?;
            let local = central_local_offset(&header, &extra)
                .ok_or_else(|| invalid_data("ローカルヘッダの位置を取得できません"))?;

            clear_flag_bit(stream, pos + 9)?; // セントラル: 汎用フラグ(+8)の上位バイト
            let mut signature = [0u8; 4];
            read_at(stream, local, &mut signature)?;
            if u32::from_le_bytes(signature) != 0x0403_4b50 {
                return Err(invalid_data("ローカルヘッダのシグネチャが不正です"));
            }
            clear_flag_bit(stream, local + 7)?; // ローカル: 汎用フラグ(+6)の上位バイト
        }
        pos += 46 + name_len + extra_len + comment_len;
    }
    stream.flush()
}

/// 上位バイトの、ビット11(= 上位バイトのビット3)を落とす。
fn clear_flag_bit<S: Read + Write + Seek>(stream: &mut S, at: u64) -> io::Result<()> {
    let mut byte = [0u8; 1];
    read_at(stream, at, &mut byte)?;
    byte[0] &= !0x08;
    stream.seek(SeekFrom::Start(at))?;
    stream.write_all(&byte)
}

fn read_at<S: Read + Seek>(stream: &mut S, at: u64, buf: &mut [u8]) -> io::Result<()> {
    stream.seek(SeekFrom::Start(at))?;
    stream.read_exact(buf)
}

fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

/// セントラルディレクトリの(エントリ数, 開始位置)。Zip64にも対応する。
fn central_directory_location<S: Read + Seek>(stream: &mut S) -> io::Result<(u64, u64)> {
    const EOCD_SIZE: u64 = 22;
    let len = stream.seek(SeekFrom::End(0))?;
    if len < EOCD_SIZE {
        return Err(invalid_data("Zipとして短すぎます"));
    }
    // EOCDは、末尾から(コメントの最大長65535を含めて)探す。
    let tail_len = len.min(EOCD_SIZE + u64::from(u16::MAX));
    let tail_start = len - tail_len;
    let mut tail = vec![0u8; tail_len as usize];
    read_at(stream, tail_start, &mut tail)?;
    let idx = (0..=tail.len() - EOCD_SIZE as usize)
        .rev()
        .find(|&i| u32_at(&tail, i) == 0x0605_4b50)
        .ok_or_else(|| invalid_data("EOCDが見つかりません"))?;
    let eocd_pos = tail_start + idx as u64;

    let count16 = u16_at(&tail, idx + 10);
    let offset32 = u32_at(&tail, idx + 16);
    if count16 != u16::MAX && offset32 != u32::MAX {
        return Ok((u64::from(count16), u64::from(offset32)));
    }

    // Zip64: EOCDの20バイト前に、Zip64 EOCDロケータがある。
    let locator_pos = eocd_pos
        .checked_sub(20)
        .ok_or_else(|| invalid_data("Zip64 EOCDロケータがありません"))?;
    let mut locator = [0u8; 20];
    read_at(stream, locator_pos, &mut locator)?;
    if u32_at(&locator, 0) != 0x0706_4b50 {
        return Err(invalid_data("Zip64 EOCDロケータのシグネチャが不正です"));
    }
    let record_pos = u64_at(&locator, 8);
    let mut record = [0u8; 56];
    read_at(stream, record_pos, &mut record)?;
    if u32_at(&record, 0) != 0x0606_4b50 {
        return Err(invalid_data("Zip64 EOCDのシグネチャが不正です"));
    }
    Ok((u64_at(&record, 32), u64_at(&record, 48)))
}

/// セントラルヘッダ(固定部46バイトと、拡張フィールド)から、ローカルヘッダの位置を求める。
///
/// 位置が `0xFFFFFFFF` のときは、Zip64拡張フィールド(ID `0x0001`)に入っている。
/// そのフィールドには、固定部が `0xFFFFFFFF` のものだけが、(非圧縮サイズ → 圧縮サイズ → 位置)の順で並ぶ。
fn central_local_offset(header: &[u8; 46], extra: &[u8]) -> Option<u64> {
    let local32 = u32_at(header, 42);
    if local32 != u32::MAX {
        return Some(u64::from(local32));
    }
    let mut skip = 0;
    if u32_at(header, 24) == u32::MAX {
        skip += 8; // 非圧縮サイズ
    }
    if u32_at(header, 20) == u32::MAX {
        skip += 8; // 圧縮サイズ
    }

    let mut at = 0;
    while at + 4 <= extra.len() {
        let id = u16_at(extra, at);
        let size = usize::from(u16_at(extra, at + 2));
        let data = extra.get(at + 4..at + 4 + size)?;
        if id == 0x0001 {
            return data.get(skip..skip + 8).map(|b| u64_at(b, 0));
        }
        at += 4 + size;
    }
    None
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::{Charset, Detection};
    use crate::normalize::Platform;
    use crate::test_util::{read_central_directory, read_made_by_hosts};
    use crate::zip_reader::{ExtractOptions, ExtractReport, extract, inspect};
    use std::io::Cursor;
    use tempfile::TempDir;
    use unicode_normalization::UnicodeNormalization;

    /// ワークフォルダ代わりの一時フォルダに、ファイルを作る。名前が `/` で終わるものはフォルダ。
    fn make_src(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (name, content) in files {
            let path = dir.path().join(name);
            if name.ends_with('/') {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, content).unwrap();
            }
        }
        dir
    }

    fn opts<'a>(
        target: TargetOs,
        encryption: Option<(EncryptionMethod, &'a str)>,
        cancel: &'a CancelToken,
    ) -> CompressOptions<'a> {
        CompressOptions { target, encryption, cancel }
    }

    fn run(
        src: &TempDir,
        out: &TempDir,
        target: TargetOs,
        encryption: Option<(EncryptionMethod, &str)>,
    ) -> (PathBuf, CompressReport) {
        let cancel = CancelToken::new();
        let dest = out.path().join("out.zip");
        let report = compress(src.path(), &dest, &opts(target, encryption, &cancel), &mut |_| {}).unwrap();
        (dest, report)
    }

    /// Zipの(フラグ, 名前のバイト列)の一覧。
    fn central(zip: &Path) -> Vec<(u16, Vec<u8>)> {
        read_central_directory(&fs::read(zip).unwrap())
    }

    fn unzip(zip: &Path, password: Option<&str>) -> ExtractReport {
        let cancel = CancelToken::new();
        let options = ExtractOptions { charset: None, password, platform: Platform::Other, cancel: &cancel };
        extract(zip, &options, &mut |_| {}).unwrap()
    }

    /// 展開先フォルダの中を、(相対パス, 中身)の一覧にする。フォルダは `/` 付き、ソート済み。
    fn tree(root: &Path) -> Vec<(String, String)> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
            for e in fs::read_dir(dir).unwrap() {
                let e = e.unwrap();
                let rel = e.path().strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                if e.path().is_dir() {
                    out.push((format!("{rel}/"), String::new()));
                    walk(base, &e.path(), out);
                } else {
                    out.push((rel, fs::read_to_string(e.path()).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    const UTF8_FLAG: u16 = 0x0800;

    #[test]
    fn 自動モードはutf8でフラグを立て_往復できる() {
        let src = make_src(&[
            ("テスト.txt", "a"),
            ("議事録/第1回.txt", "b"),
            ("English.txt", "c"),
            ("空のフォルダ/", ""),
        ]);
        let out = TempDir::new().unwrap();
        let (zip, report) = run(&src, &out, TargetOs::Auto, None);
        assert_eq!(report.file_count, 3);
        assert!(report.skipped.is_empty() && report.renamed.is_empty());

        for (flags, name) in central(&zip) {
            let non_ascii = !name.is_ascii();
            assert_eq!(flags & UTF8_FLAG != 0, non_ascii, "{}", String::from_utf8_lossy(&name));
        }

        let extracted = unzip(&zip, None);
        assert_eq!(
            tree(&extracted.out_dir),
            [
                ("English.txt".to_string(), "c".to_string()),
                ("テスト.txt".to_string(), "a".to_string()),
                ("空のフォルダ/".to_string(), String::new()),
                ("議事録/".to_string(), String::new()),
                ("議事録/第1回.txt".to_string(), "b".to_string()),
            ]
        );
    }

    #[test]
    fn windowsモードの作成元osはfat_自動はunix() {
        // 7-Zipは、作成元がUnixのエントリの、フラグなしの名前をUTF-8として読む。
        // Shift_JISの名前が化けないよう、「Windows」では7-Zipと同じFAT(0)にする。
        let src = make_src(&[("日本語.txt", "1"), ("dir/", ""), ("dir/a.txt", "2")]);
        let out = TempDir::new().unwrap();
        let (win, _) = run(&src, &out, TargetOs::Windows, None);
        assert_eq!(read_made_by_hosts(&fs::read(&win).unwrap()), [0, 0, 0]);
        let (auto, _) = run(&src, &out, TargetOs::Auto, None);
        assert_eq!(read_made_by_hosts(&fs::read(&auto).unwrap()), [3, 3, 3]);
        // FATでも、読み戻せる(フォルダも、ファイルも)
        let extracted = unzip(&win, None);
        assert!(extracted.out_dir.join("日本語.txt").is_file());
        assert!(extracted.out_dir.join("dir").is_dir());
        assert!(extracted.out_dir.join("dir/a.txt").is_file());
    }

    #[test]
    fn nfdの名前はnfcで書く() {
        let nfd: String = "がぎぐ.txt".nfd().collect();
        let src = make_src(&[(&nfd, "x")]);
        let out = TempDir::new().unwrap();
        for target in [TargetOs::Auto, TargetOs::Windows] {
            let (zip, report) = run(&src, &out, target, None);
            assert!(report.renamed.is_empty(), "NFC正規化だけなら通知しない");
            let names = central(&zip);
            assert_eq!(names.len(), 1);
            let decoded = if target == TargetOs::Auto {
                String::from_utf8(names[0].1.clone()).unwrap()
            } else {
                encoding_rs::SHIFT_JIS.decode(&names[0].1).0.into_owned()
            };
            assert_eq!(decoded, "がぎぐ.txt");
        }
    }

    #[test]
    fn windowsモードはshift_jisで書き_変換できない名前だけutf8フラグ付きにする() {
        let src = make_src(&[
            ("日本語.txt", "1"),
            ("楽しい😀写真.jpg", "2"),
            ("ｱｲｳ①㈱.txt", "3"),
            ("plain.txt", "4"),
        ]);
        let out = TempDir::new().unwrap();
        let (zip, report) = run(&src, &out, TargetOs::Windows, None);
        assert!(report.renamed.is_empty(), "`_` への置き換えはしない");

        let entries = central(&zip);
        let find = |bytes: &[u8]| entries.iter().find(|(_, n)| n == bytes).cloned();
        // Shift_JISで書く(フラグなし)
        let (flags, _) = find(&[0x93, 0xFA, 0x96, 0x7B, 0x8C, 0xEA, b'.', b't', b'x', b't']).expect("日本語.txt");
        assert_eq!(flags & UTF8_FLAG, 0);
        // 変換できない名前は、元のままUTF-8(フラグあり)
        let (flags, _) = find("楽しい😀写真.jpg".as_bytes()).expect("絵文字");
        assert_ne!(flags & UTF8_FLAG, 0);
        // ASCIIはフラグなし
        let (flags, _) = find(b"plain.txt").expect("plain");
        assert_eq!(flags & UTF8_FLAG, 0);

        // 読み戻すと、元の名前に戻る(フラグ付きは判定から外れ、残りはShift_JISで確定する)
        assert_eq!(
            inspect(&zip).unwrap().detection,
            Detection::Resolved { charset: Charset::ShiftJis }
        );
        let extracted = unzip(&zip, None);
        let mut names: Vec<String> = tree(&extracted.out_dir).into_iter().map(|(n, _)| n).collect();
        names.sort();
        let mut expected: Vec<String> = ["plain.txt", "ｱｲｳ①㈱.txt", "日本語.txt", "楽しい😀写真.jpg"].map(String::from).to_vec();
        expected.sort();
        assert_eq!(names, expected);
    }

    #[test]
    fn shift_jisが偶然utf8として正しい名前もフラグを落とす() {
        // 半角カナ「ﾃｱ」はShift_JISで `C3 B1`。UTF-8の「ñ」として正しいので、zipクレートはフラグを立ててしまう。
        let src = make_src(&[("ﾃｱ.txt", "x"), ("日本語.txt", "y")]);
        let out = TempDir::new().unwrap();
        let (zip, _) = run(&src, &out, TargetOs::Windows, None);
        let entries = central(&zip);
        let (flags, name) = entries
            .iter()
            .find(|(_, n)| n.starts_with(&[0xC3, 0xB1]))
            .expect("ﾃｱ.txt");
        assert_eq!(name[..2], [0xC3, 0xB1]);
        assert_eq!(flags & UTF8_FLAG, 0, "フラグが落ちている");
        let extracted = unzip(&zip, None);
        assert!(extracted.out_dir.join("ﾃｱ.txt").is_file());
    }

    #[test]
    fn 波ダッシュは全角チルダにして一覧に載せる() {
        let src = make_src(&[("第1回〜第3回.txt", "x"), ("第1回～第3回.txt", "y")]);
        let out = TempDir::new().unwrap();
        let (zip, report) = run(&src, &out, TargetOs::Windows, None);
        assert_eq!(report.file_count, 2);
        // 〜 → ～ で同名になる。名前順で先に来る `〜`(U+301C)が `～` の名前を取り、
        // 元から `～`(U+FF5E)だった方に連番が付く。どちらもShift_JISで書ける。
        assert_eq!(
            report.renamed,
            [
                RenamedEntry { from: "第1回〜第3回.txt".into(), to: "第1回～第3回.txt".into() },
                RenamedEntry { from: "第1回～第3回.txt".into(), to: "第1回～第3回_1.txt".into() },
            ]
        );
        for (flags, _) in central(&zip) {
            assert_eq!(flags & UTF8_FLAG, 0);
        }
        let extracted = unzip(&zip, None);
        let names: Vec<String> = tree(&extracted.out_dir).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["第1回～第3回.txt", "第1回～第3回_1.txt"]);
    }

    #[test]
    fn 波ダッシュだけの名前は置き換えを通知する() {
        let src = make_src(&[("第1回〜第3回.txt", "x")]);
        let out = TempDir::new().unwrap();
        let (_, report) = run(&src, &out, TargetOs::Windows, None);
        assert_eq!(
            report.renamed,
            [RenamedEntry { from: "第1回〜第3回.txt".into(), to: "第1回～第3回.txt".into() }]
        );
        // 自動モードでは置き換えない
        let (zip, report) = run(&src, &out, TargetOs::Auto, None);
        assert!(report.renamed.is_empty());
        assert!(central(&zip)[0].1 == "第1回〜第3回.txt".as_bytes());
    }

    #[test]
    fn os由来の不要ファイルは含めない() {
        let src = make_src(&[
            (".DS_Store", "x"),
            ("__MACOSX/._a.txt", "x"),
            ("d/Thumbs.db", "x"),
            ("d/desktop.ini", "x"),
            ("d/keep.txt", "ok"),
        ]);
        let out = TempDir::new().unwrap();
        let (zip, report) = run(&src, &out, TargetOs::Auto, None);
        assert_eq!(report.file_count, 1);
        assert!(report.skipped.is_empty());
        let names: Vec<String> = central(&zip).into_iter().map(|(_, n)| String::from_utf8(n).unwrap()).collect();
        assert_eq!(names, ["d/", "d/keep.txt"]);
    }

    #[test]
    fn パスワード付きで書ける() {
        for method in [EncryptionMethod::ZipCrypto, EncryptionMethod::Aes256] {
            let src = make_src(&[("機密.txt", "中身"), ("dir/", ""), ("dir/b.txt", "bb")]);
            let out = TempDir::new().unwrap();
            let (zip, _) = run(&src, &out, TargetOs::Auto, Some((method, "秘密pw")));

            let info = inspect(&zip).unwrap();
            assert!(info.encrypted, "{method:?}");

            // パスワード無し・違うパスワードでは展開できない
            let cancel = CancelToken::new();
            let no_pw = ExtractOptions { charset: None, password: None, platform: Platform::Other, cancel: &cancel };
            assert!(matches!(extract(&zip, &no_pw, &mut |_| {}), Err(AppError::PasswordRequired)));
            let bad = ExtractOptions { charset: None, password: Some("x"), platform: Platform::Other, cancel: &cancel };
            assert!(matches!(extract(&zip, &bad, &mut |_| {}), Err(AppError::WrongPassword)), "{method:?}");

            let extracted = unzip(&zip, Some("秘密pw"));
            let t = tree(&extracted.out_dir);
            assert!(t.contains(&("機密.txt".to_string(), "中身".to_string())), "{method:?}");
            assert!(t.contains(&("dir/b.txt".to_string(), "bb".to_string())), "{method:?}");
        }
    }

    #[test]
    fn パスワードが空なら暗号化しない() {
        let src = make_src(&[("a.txt", "x")]);
        let out = TempDir::new().unwrap();
        let (zip, _) = run(&src, &out, TargetOs::Auto, Some((EncryptionMethod::Aes256, "")));
        assert!(!inspect(&zip).unwrap().encrypted);
    }

    #[test]
    fn パスワード付きのwindowsモードでもフラグが正しい() {
        for method in [EncryptionMethod::ZipCrypto, EncryptionMethod::Aes256] {
            let src = make_src(&[("日本語.txt", "1"), ("😀.txt", "2"), ("ﾃｱ.txt", "3")]);
            let out = TempDir::new().unwrap();
            let (zip, _) = run(&src, &out, TargetOs::Windows, Some((method, "pw")));
            for (flags, name) in central(&zip) {
                let utf8 = std::str::from_utf8(&name).is_ok() && !name.is_ascii() && name.starts_with("😀".as_bytes());
                assert_eq!(flags & UTF8_FLAG != 0, utf8, "{method:?} {name:?}");
            }
            let extracted = unzip(&zip, Some("pw"));
            assert_eq!(tree(&extracted.out_dir).len(), 3, "{method:?}");
        }
    }

    #[test]
    fn キャンセルすると一時ファイルも保存先も残さない() {
        let src = make_src(&[("a.txt", "x")]);
        let out = TempDir::new().unwrap();
        let dest = out.path().join("out.zip");
        let cancel = CancelToken::new();
        cancel.cancel();
        let result = compress(src.path(), &dest, &opts(TargetOs::Auto, None, &cancel), &mut |_| {});
        assert!(matches!(result, Err(AppError::Cancelled)));
        assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
    }

    #[test]
    fn 途中でキャンセルしても既存のzipは壊さない() {
        let big = "z".repeat(1024 * 1024);
        let src = make_src(&[("1.bin", &big), ("2.bin", &big)]);
        let out = TempDir::new().unwrap();
        let dest = out.path().join("out.zip");
        fs::write(&dest, b"old content").unwrap();

        let cancel = CancelToken::new();
        let result = compress(src.path(), &dest, &opts(TargetOs::Auto, None, &cancel), &mut |p| {
            if p.done > 0 {
                cancel.cancel();
            }
        });
        assert!(matches!(result, Err(AppError::Cancelled)));
        assert_eq!(fs::read(&dest).unwrap(), b"old content");
        assert_eq!(fs::read_dir(out.path()).unwrap().count(), 1, "一時ファイルは残らない");
    }

    #[test]
    fn 保存先に同名のファイルがあれば置き換える() {
        let src = make_src(&[("a.txt", "new")]);
        let out = TempDir::new().unwrap();
        fs::write(out.path().join("out.zip"), b"old").unwrap();
        let (zip, _) = run(&src, &out, TargetOs::Auto, None);
        let extracted = unzip(&zip, None);
        assert_eq!(tree(&extracted.out_dir), [("a.txt".to_string(), "new".to_string())]);
        assert_eq!(fs::read_dir(out.path()).unwrap().filter(|e| {
            e.as_ref().unwrap().file_name().to_string_lossy().ends_with("tmp")
        }).count(), 0);
    }

    #[test]
    fn 進捗は最後に全体に達する() {
        let data = "q".repeat(300_000);
        let src = make_src(&[("a.bin", &data), ("b.bin", &data)]);
        let out = TempDir::new().unwrap();
        let cancel = CancelToken::new();
        let mut seen: Vec<Progress> = Vec::new();
        compress(src.path(), &out.path().join("o.zip"), &opts(TargetOs::Auto, None, &cancel), &mut |p| seen.push(p)).unwrap();
        assert_eq!(seen.first(), Some(&Progress { done: 0, total: 600_000 }));
        assert_eq!(seen.last(), Some(&Progress { done: 600_000, total: 600_000 }));
    }

    #[cfg(unix)]
    #[test]
    fn シンボリックリンクは追加せずスキップ一覧に載せる() {
        let src = make_src(&[("real.txt", "x")]);
        std::os::unix::fs::symlink("/etc/hosts", src.path().join("link")).unwrap();
        let out = TempDir::new().unwrap();
        let (zip, report) = run(&src, &out, TargetOs::Auto, None);
        assert_eq!(report.file_count, 1);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].name, "link");
        assert_eq!(central(&zip).len(), 1);
    }

    #[test]
    fn 更新日時が入る() {
        let src = make_src(&[("a.txt", "x")]);
        let out = TempDir::new().unwrap();
        let (zip, _) = run(&src, &out, TargetOs::Auto, None);
        let archive = zip::ZipArchive::new(File::open(&zip).unwrap()).unwrap();
        let mut archive = archive;
        let entry = archive.by_index(0).unwrap();
        let year = entry.last_modified().map(|d| d.year()).unwrap_or(0);
        assert!(year >= 2024, "year = {year}");
    }

    /// 他のツール(unzip、7-Zip、Pythonなど)で読めるか手で確認するためのサンプルを書き出す。
    /// `UNI_ZIPPER_SAMPLE_DIR=/path cargo test write_samples -- --ignored` で実行する。
    #[test]
    #[ignore]
    fn write_samples() {
        let dir = PathBuf::from(std::env::var("UNI_ZIPPER_SAMPLE_DIR").expect("UNI_ZIPPER_SAMPLE_DIR"));
        let nfd: String = "がぎぐ.txt".nfd().collect();
        let src = make_src(&[
            ("日本語.txt", "1"),
            ("議事録/第1回.txt", "2"),
            (&nfd, "3"),
            ("第1回〜第3回.txt", "4"),
            ("ｱｲｳ①㈱.txt", "5"),
            ("ﾃｱ.txt", "6"),
            ("楽しい😀写真.jpg", "7"),
            ("空/", ""),
        ]);
        let cancel = CancelToken::new();
        for (name, target, enc) in [
            ("auto.zip", TargetOs::Auto, None),
            ("windows.zip", TargetOs::Windows, None),
            ("windows_zipcrypto.zip", TargetOs::Windows, Some((EncryptionMethod::ZipCrypto, "pw"))),
            ("auto_aes.zip", TargetOs::Auto, Some((EncryptionMethod::Aes256, "pw"))),
            ("auto_zipcrypto.zip", TargetOs::Auto, Some((EncryptionMethod::ZipCrypto, "pw"))),
        ] {
            compress(src.path(), &dir.join(name), &opts(target, enc, &cancel), &mut |_| {}).unwrap();
        }
    }

    /// 診断用: ASCII名だけのサンプル(`UNI_ZIPPER_SAMPLE_DIR`)。
    #[test]
    #[ignore]
    fn write_samples_ascii() {
        let dir = PathBuf::from(std::env::var("UNI_ZIPPER_SAMPLE_DIR").expect("UNI_ZIPPER_SAMPLE_DIR"));
        let src = make_src(&[("a.txt", "hello world hello world"), ("b.txt", "second file")]);
        let cancel = CancelToken::new();
        for (name, target, enc) in [
            ("ascii_plain.zip", TargetOs::Auto, None),
            ("ascii_zipcrypto.zip", TargetOs::Auto, Some((EncryptionMethod::ZipCrypto, "pw"))),
        ] {
            compress(src.path(), &dir.join(name), &opts(target, enc, &cancel), &mut |_| {}).unwrap();
        }
    }

    // ---- UTF-8フラグの後処理(部品) ----

    /// ヘッダを手で組み立てた最小のZip(エントリ名だけで、データなし)。
    fn tiny_zip(entries: &[(&[u8], u16)]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut locals = Vec::new();
        for (name, flags) in entries {
            locals.push(buf.len() as u32);
            buf.extend(0x0403_4b50u32.to_le_bytes());
            buf.extend(20u16.to_le_bytes()); // version
            buf.extend(flags.to_le_bytes());
            buf.extend([0u8; 2 + 2 + 2 + 4 + 4 + 4]); // method, time, date, crc, sizes
            buf.extend((name.len() as u16).to_le_bytes());
            buf.extend(0u16.to_le_bytes());
            buf.extend(*name);
        }
        let cd_start = buf.len() as u32;
        for ((name, flags), local) in entries.iter().zip(&locals) {
            buf.extend(0x0201_4b50u32.to_le_bytes());
            buf.extend(20u16.to_le_bytes());
            buf.extend(20u16.to_le_bytes());
            buf.extend(flags.to_le_bytes());
            buf.extend([0u8; 2 + 2 + 2 + 4 + 4 + 4]);
            buf.extend((name.len() as u16).to_le_bytes());
            buf.extend([0u8; 2 + 2 + 2 + 2 + 4]); // extra, comment, disk, internal, external
            buf.extend(local.to_le_bytes());
            buf.extend(*name);
        }
        let cd_size = buf.len() as u32 - cd_start;
        buf.extend(0x0605_4b50u32.to_le_bytes());
        buf.extend([0u8; 4]);
        buf.extend((entries.len() as u16).to_le_bytes());
        buf.extend((entries.len() as u16).to_le_bytes());
        buf.extend(cd_size.to_le_bytes());
        buf.extend(cd_start.to_le_bytes());
        buf.extend(0u16.to_le_bytes());
        buf
    }

    #[test]
    fn 指定したエントリだけフラグを落とす() {
        let buf = tiny_zip(&[(b"a", 0x0800), (b"b", 0x0800 | 0x0001), (b"c", 0x0800)]);
        let mut cursor = Cursor::new(buf);
        clear_utf8_flags(&mut cursor, &[true, false, true]).unwrap();
        let flags: Vec<u16> = read_central_directory(cursor.get_ref()).into_iter().map(|(f, _)| f).collect();
        assert_eq!(flags, [0x0000, 0x0801, 0x0000]);
        // ローカルヘッダも落ちている(+6)
        let b = cursor.get_ref();
        assert_eq!(u16::from_le_bytes([b[6], b[7]]), 0x0000);
    }

    #[test]
    fn 対象が無ければ何もしない() {
        let buf = tiny_zip(&[(b"a", 0x0800)]);
        let mut cursor = Cursor::new(buf.clone());
        clear_utf8_flags(&mut cursor, &[false]).unwrap();
        assert_eq!(cursor.into_inner(), buf);
    }

    #[test]
    fn エントリ数が合わなければエラー() {
        let mut cursor = Cursor::new(tiny_zip(&[(b"a", 0x0800)]));
        assert!(clear_utf8_flags(&mut cursor, &[true, true]).is_err());
    }

    #[test]
    fn zip64のローカルヘッダ位置を拡張フィールドから取れる() {
        let mut header = [0u8; 46];
        let extra_with = |values: &[u64]| {
            let mut e = Vec::new();
            e.extend(0x0001u16.to_le_bytes());
            e.extend(((values.len() * 8) as u16).to_le_bytes());
            for v in values {
                e.extend(v.to_le_bytes());
            }
            e
        };
        // 位置が通常の値なら、そのまま
        header[42..46].copy_from_slice(&1234u32.to_le_bytes());
        assert_eq!(central_local_offset(&header, &[]), Some(1234));

        // 位置だけが 0xFFFFFFFF
        header[42..46].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(central_local_offset(&header, &extra_with(&[5_000_000_000])), Some(5_000_000_000));

        // 非圧縮サイズ・圧縮サイズも 0xFFFFFFFF なら、位置は3番目
        header[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        header[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            central_local_offset(&header, &extra_with(&[111, 222, 6_000_000_000])),
            Some(6_000_000_000)
        );

        // 別のフィールドが先にあっても読める / フィールドが無ければ None
        let mut extra = vec![0x55, 0x54, 2, 0, 9, 9];
        extra.extend(extra_with(&[1, 2, 7_000_000_000]));
        assert_eq!(central_local_offset(&header, &extra), Some(7_000_000_000));
        assert_eq!(central_local_offset(&header, &[0x55, 0x54, 2, 0, 9, 9]), None);
    }

    #[test]
    fn zip64形式のzipでも名前の判定とフラグ処理が動く() {
        // 1ファイルをZip64指定で書く(拡張フィールド付き)。位置は通常の値のまま。
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = ZipWriter::new(&mut buf);
            let o: FileOptions<'_, '_, ()> = SimpleFileOptions::default().large_file(true);
            w.start_file_with_options(o.name(&[0x93, 0xFA, 0x96, 0x7B])).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        clear_utf8_flags(&mut buf, &[true]).unwrap();
        let entries = read_central_directory(buf.get_ref());
        assert_eq!(entries[0].0 & UTF8_FLAG, 0);
    }
}
