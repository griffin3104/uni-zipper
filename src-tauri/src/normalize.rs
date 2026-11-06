//! ファイル名の正規化・無害化(docs/技術設計.md 5.3 / 6.1)。
//!
//! - 解凍側: NFC正規化、Zip Slip対策、Windowsで使えない文字・予約名の置き換え。
//! - 圧縮側: 「Windows」モード用に、名前の書き方を決める(7-Zip方式。波ダッシュ → 全角チルダにしてから、
//!   Shift_JISに変換できればShift_JIS、できなければ元の名前のままUTF-8)。同名になったときの連番付け。

#![allow(dead_code)] // 解凍・圧縮から使われるまでの暫定(実装ステップ③④)

use std::collections::HashSet;

use unicode_normalization::UnicodeNormalization;

use crate::encoding::encode_shift_jis_strict;
use crate::workdir::numbered_name;

/// 波ダッシュ(Macで出やすい。Shift_JISに無い)
const WAVE_DASH: char = '\u{301C}';
/// 全角チルダ(Windows(CP932)で波ダッシュに対応する文字)
const FULLWIDTH_TILDE: char = '\u{FF5E}';

/// 濁点・半濁点を結合した形(NFC)にする。Macの分解形(NFD)を直すために使う。
pub fn to_nfc(s: &str) -> String {
    s.nfc().collect()
}

// ---------------------------------------------------------------------------
// 解凍側
// ---------------------------------------------------------------------------

/// 展開先のOS。Windowsだけ、使えない文字・予約名を置き換える。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    /// macOS(Linuxも含む)
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) { Self::Windows } else { Self::Other }
    }
}

/// 展開してよいと判断したエントリのパス。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafePath {
    /// `/` 区切り、先頭・末尾に `/` なし。NFC正規化済みで、OSに合わせて無害化済み。
    pub path: String,
    /// 元の名前の末尾が区切り文字だった(= ディレクトリのエントリ)。
    pub is_dir: bool,
    /// 使えない文字などを置き換えた(NFC正規化だけの場合は含めない)。
    pub replaced: bool,
}

/// 展開してはいけないエントリの理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsafeReason {
    /// `..` を含み、展開先の外に出られる(Zip Slip)。
    Traversal,
    /// 絶対パス(`/` や `\` で始まる、`C:` などのドライブ指定)。
    Absolute,
    /// 名前が空、または `.` だけ。
    Empty,
}

/// Zipのエントリ名を、展開先に安全に書ける相対パスにする。
///
/// 区切りは `/` と `\` の両方を区切りとして扱う(古いZipは `\` を使うため)。
/// 空の階層と `.` は取り除く。
pub fn sanitize_entry_path(raw: &str, platform: Platform) -> Result<SafePath, UnsafeReason> {
    let nfc = to_nfc(raw);
    if nfc.starts_with(['/', '\\']) {
        return Err(UnsafeReason::Absolute);
    }
    let is_dir = nfc.ends_with(['/', '\\']);

    let mut parts = Vec::new();
    let mut replaced = false;
    for (i, part) in nfc.split(['/', '\\']).enumerate() {
        match part {
            "" | "." => continue,
            ".." => return Err(UnsafeReason::Traversal),
            _ => {}
        }
        if i == 0 && is_drive_prefix(part) {
            return Err(UnsafeReason::Absolute);
        }
        let (clean, changed) = sanitize_component(part, platform);
        replaced |= changed;
        parts.push(clean);
    }

    if parts.is_empty() {
        return Err(UnsafeReason::Empty);
    }
    Ok(SafePath { path: parts.join("/"), is_dir, replaced })
}

/// `C:` のようなドライブ指定か。
fn is_drive_prefix(part: &str) -> bool {
    let mut chars = part.chars();
    matches!((chars.next(), chars.next(), chars.next()), (Some(c), Some(':'), None) if c.is_ascii_alphabetic())
}

/// Windowsの予約名(拡張子の有無、大文字小文字を問わない)。
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 1階層分の名前を無害化する。戻り値は(新しい名前, 置き換えたか)。
///
/// - 共通: NUL文字を `_` にする。
/// - Windows: `< > : " | ? *` と制御文字を `_` にする。末尾の `.` と空白は `_` にする。
///   予約名(`CON`、`NUL`、`COM1` など)は、末尾に `_` を付ける(`CON.txt` → `CON_.txt`)。
pub fn sanitize_component(name: &str, platform: Platform) -> (String, bool) {
    let mut out: Vec<char> = name
        .chars()
        .map(|c| match platform {
            Platform::Windows if is_windows_invalid(c) => '_',
            _ if c == '\0' => '_',
            _ => c,
        })
        .collect();

    if platform == Platform::Windows {
        for c in out.iter_mut().rev() {
            if *c == '.' || *c == ' ' {
                *c = '_';
            } else {
                break;
            }
        }
    }
    let mut result: String = out.into_iter().collect();

    if platform == Platform::Windows {
        let stem = result.split('.').next().unwrap_or("");
        if WINDOWS_RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
            result.insert(stem.len(), '_');
        }
    }

    let changed = result != name;
    (result, changed)
}

fn is_windows_invalid(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (c as u32) < 0x20
}

// ---------------------------------------------------------------------------
// 圧縮側
// ---------------------------------------------------------------------------

/// エントリ名のバイト列の文字コード。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameEncoding {
    /// Shift_JIS(UTF-8フラグなしで書く)。
    ShiftJis,
    /// UTF-8(UTF-8フラグ付きで書く)。
    Utf8,
}

/// 「Windows」モードで書き込む名前。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedName {
    /// Zipに書くバイト列。
    pub bytes: Vec<u8>,
    /// バイト列に対応する文字列(`〜` の置き換え後。`_` への置き換えはしない)。
    pub text: String,
    pub encoding: NameEncoding,
    /// 波ダッシュを全角チルダに置き換えた。
    pub replaced: bool,
}

/// 「Windows」モードの名前の、文字としての準備。戻り値は(名前, 置き換えたか)。
///
/// 1. NFCに正規化する(NFC正規化だけでは「置き換えた」扱いにしない)。
/// 2. 波ダッシュ `〜`(U+301C)を、全角チルダ `～`(U+FF5E)にする(置き換えたら `true`)。
///
/// 同名の連番付け(`dedupe_paths`)は、この結果に対して行ってから、`encode_windows_name` で書き方を決める。
pub fn windows_text(name: &str) -> (String, bool) {
    let mut replaced = false;
    let text = to_nfc(name)
        .chars()
        .map(|c| {
            if c == WAVE_DASH {
                replaced = true;
                FULLWIDTH_TILDE
            } else {
                c
            }
        })
        .collect();
    (text, replaced)
}

/// 名前の書き方を決める。Shift_JISに全文字を変換できればShift_JIS、1文字でも変換できなければ、
/// 名前はそのままUTF-8で書く(文字を `_` に置き換えない)。
///
/// `¥`(U+00A5)と `‾`(U+203E)は変換できない文字として扱うので、含む名前はUTF-8になる
/// (理由は `encoding::encode_shift_jis_strict`)。
pub fn encode_windows_name(text: &str) -> (Vec<u8>, NameEncoding) {
    match encode_shift_jis_strict(text) {
        Some(bytes) => (bytes, NameEncoding::ShiftJis),
        None => (text.as_bytes().to_vec(), NameEncoding::Utf8),
    }
}

/// 名前1つについて、`windows_text` と `encode_windows_name` をまとめて行う(連番付けが要らないとき用)。
pub fn prepare_windows_name(name: &str) -> EncodedName {
    let (text, replaced) = windows_text(name);
    let (bytes, encoding) = encode_windows_name(&text);
    EncodedName { bytes, text, encoding, replaced }
}

/// Zip内のパス一覧で、同じ名前になったものに連番を付ける(`a.txt`、`a_1.txt`、`a_2.txt`)。
///
/// 末尾が `/` のものはディレクトリとして扱い、拡張子とみなさずに末尾へ連番を付ける。
/// 先に出たものの名前は変えない。
/// 大文字小文字は区別せずに比べる(macOS・Windowsは `A.txt` と `a.txt` を同じファイルとして扱うため)。
pub fn dedupe_paths(paths: &[String]) -> Vec<String> {
    let mut used: HashSet<String> = HashSet::new();
    let mut result = Vec::with_capacity(paths.len());

    for path in paths {
        let (body, is_dir) = match path.strip_suffix('/') {
            Some(body) => (body, true),
            None => (path.as_str(), false),
        };
        let (parent, last) = match body.rfind('/') {
            Some(i) => (&body[..=i], &body[i + 1..]),
            None => ("", body),
        };
        let suffix = if is_dir { "/" } else { "" };

        let mut candidate = path.clone();
        let mut n = 0;
        while used.contains(&candidate.to_lowercase()) {
            n += 1;
            candidate = format!("{parent}{}{suffix}", numbered_name(last, n, is_dir));
        }
        used.insert(candidate.to_lowercase());
        result.push(candidate);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use Platform::{Other, Windows};

    fn safe(raw: &str, platform: Platform) -> String {
        sanitize_entry_path(raw, platform).unwrap().path
    }

    // ---- NFC ----

    #[test]
    fn 濁点と半濁点の分離を結合する() {
        let nfd = "か\u{3099}き\u{3099}.txt"; // がぎ
        let handan = "は\u{309A}.txt"; // ぱ
        assert_eq!(to_nfc(nfd), "がぎ.txt");
        assert_eq!(to_nfc(handan), "ぱ.txt");
        assert_eq!(to_nfc("がぎ.txt"), "がぎ.txt"); // 既にNFCなら変わらない
        assert_eq!(to_nfc("abc"), "abc");
    }

    // ---- 解凍: パス ----

    #[test]
    fn 通常のパスはそのまま通りnfcになる() {
        assert_eq!(safe("a/b/c.txt", Other), "a/b/c.txt");
        assert_eq!(safe("日本語/ファイル.txt", Windows), "日本語/ファイル.txt");
        assert_eq!(safe("か\u{3099}/き\u{3099}.txt", Windows), "が/ぎ.txt");
    }

    #[test]
    fn nfcにしただけでは置き換え扱いにしない() {
        let p = sanitize_entry_path("か\u{3099}.txt", Windows).unwrap();
        assert_eq!(p.path, "が.txt");
        assert!(!p.replaced);
    }

    #[test]
    fn バックスラッシュも区切りとして扱う() {
        assert_eq!(safe("a\\b\\c.txt", Other), "a/b/c.txt");
        assert_eq!(safe("a/b\\c.txt", Other), "a/b/c.txt");
    }

    #[test]
    fn 空の階層とドットは取り除く() {
        assert_eq!(safe("./a//b/./c.txt", Other), "a/b/c.txt");
    }

    #[test]
    fn ディレクトリのエントリを判別できる() {
        let d = sanitize_entry_path("a/b/", Other).unwrap();
        assert_eq!((d.path.as_str(), d.is_dir), ("a/b", true));
        let f = sanitize_entry_path("a/b", Other).unwrap();
        assert!(!f.is_dir);
    }

    #[test]
    fn zip_slipを拒否する() {
        for raw in ["../evil.txt", "a/../../evil.txt", "a/..", "..\\evil.txt", "a\\..\\..\\x", ".."] {
            for platform in [Windows, Other] {
                assert_eq!(
                    sanitize_entry_path(raw, platform),
                    Err(UnsafeReason::Traversal),
                    "{raw}"
                );
            }
        }
    }

    #[test]
    fn 絶対パスを拒否する() {
        for raw in ["/etc/passwd", "\\windows\\system32", "\\\\server\\share\\a.txt", "C:\\a.txt", "c:/a.txt", "D:"] {
            assert_eq!(
                sanitize_entry_path(raw, Other),
                Err(UnsafeReason::Absolute),
                "{raw}"
            );
        }
    }

    #[test]
    fn 二階層目以降のコロンは絶対パスではない() {
        // `a/C:` は相対パス。Windowsではコロンが `_` になるだけ
        assert_eq!(safe("a/C:", Other), "a/C:");
        assert_eq!(safe("a/C:", Windows), "a/C_");
    }

    #[test]
    fn 空の名前を拒否する() {
        for raw in ["", ".", "./", "//", "/./"] {
            let expected = if raw.starts_with('/') { UnsafeReason::Absolute } else { UnsafeReason::Empty };
            assert_eq!(sanitize_entry_path(raw, Other), Err(expected), "{raw:?}");
        }
    }

    // ---- 解凍: 1階層の無害化 ----

    #[test]
    fn windowsで使えない文字を置き換える() {
        let (name, changed) = sanitize_component("a:b*c?d\"e<f>g|h.txt", Windows);
        assert_eq!(name, "a_b_c_d_e_f_g_h.txt");
        assert!(changed);
        // 制御文字
        assert_eq!(sanitize_component("a\nb\tc", Windows).0, "a_b_c");
    }

    #[test]
    fn macではwindowsの禁止文字をそのまま残す() {
        let (name, changed) = sanitize_component("a:b*c?.txt", Other);
        assert_eq!(name, "a:b*c?.txt");
        assert!(!changed);
    }

    #[test]
    fn nul文字は両方で置き換える() {
        assert_eq!(sanitize_component("a\0b", Other), ("a_b".to_string(), true));
        assert_eq!(sanitize_component("a\0b", Windows), ("a_b".to_string(), true));
    }

    #[test]
    fn windowsの末尾のドットと空白を置き換える() {
        assert_eq!(sanitize_component("name.", Windows), ("name_".to_string(), true));
        assert_eq!(sanitize_component("name ", Windows), ("name_".to_string(), true));
        assert_eq!(sanitize_component("name. .", Windows), ("name___".to_string(), true));
        // 途中のドット・空白はそのまま
        assert_eq!(sanitize_component("a. b.txt", Windows), ("a. b.txt".to_string(), false));
        assert_eq!(sanitize_component("name.", Other), ("name.".to_string(), false));
    }

    #[test]
    fn windowsの予約名は末尾にアンダースコアを付ける() {
        assert_eq!(sanitize_component("CON", Windows).0, "CON_");
        assert_eq!(sanitize_component("con.txt", Windows).0, "con_.txt");
        assert_eq!(sanitize_component("Nul.tar.gz", Windows).0, "Nul_.tar.gz");
        assert_eq!(sanitize_component("COM1", Windows).0, "COM1_");
        assert_eq!(sanitize_component("LPT9.log", Windows).0, "LPT9_.log");
        // 予約名に似ているだけのものは変えない
        for ok in ["CONSOLE", "COM10", "COM0", "AUXILIARY.txt", "a.CON", "NULL"] {
            assert_eq!(sanitize_component(ok, Windows), (ok.to_string(), false), "{ok}");
        }
        assert_eq!(sanitize_component("CON", Other), ("CON".to_string(), false));
    }

    #[test]
    fn 置き換えたかどうかをパスに反映する() {
        let p = sanitize_entry_path("a/b:c.txt", Windows).unwrap();
        assert_eq!(p.path, "a/b_c.txt");
        assert!(p.replaced);
        assert!(!sanitize_entry_path("a/bc.txt", Windows).unwrap().replaced);
    }

    // ---- 圧縮: Windows名 ----

    #[test]
    fn 通常の日本語名はそのままshift_jisになる() {
        let n = prepare_windows_name("日本語/ファイル.txt");
        assert_eq!(n.text, "日本語/ファイル.txt");
        assert!(!n.replaced);
        assert_eq!(
            n.bytes,
            [0x93, 0xFA, 0x96, 0x7B, 0x8C, 0xEA, b'/', 0x83, 0x74, 0x83, 0x40, 0x83, 0x43, 0x83, 0x8B, b'.', b't', b'x', b't']
        );
    }

    #[test]
    fn 濁点の分離はnfcにしてから変換する() {
        let n = prepare_windows_name("か\u{3099}き\u{3099}.txt");
        assert_eq!(n.text, "がぎ.txt");
        assert_eq!(n.bytes, [0x82, 0xAA, 0x82, 0xAC, b'.', b't', b'x', b't']);
        assert!(!n.replaced, "NFC正規化だけでは置き換え扱いにしない");
    }

    #[test]
    fn 波ダッシュは全角チルダに置き換える() {
        let n = prepare_windows_name("第1回〜第3回.txt");
        assert_eq!(n.text, "第1回～第3回.txt");
        assert!(n.replaced);
        // 全角チルダ U+FF5E は 81 60
        assert!(n.bytes.windows(2).any(|w| w == [0x81, 0x60]));
        assert!(!n.text.contains('_'), "波ダッシュは _ ではなく全角チルダにする");
    }

    #[test]
    fn 変換できない文字を含む名前は元のままutf8にする() {
        let n = prepare_windows_name("楽しい😀写真.jpg");
        assert_eq!(n.text, "楽しい😀写真.jpg");
        assert_eq!(n.bytes, "楽しい😀写真.jpg".as_bytes());
        assert_eq!(n.encoding, NameEncoding::Utf8);
        assert!(!n.replaced, "`_` への置き換えはしない");

        let k = prepare_windows_name("한글.txt");
        assert_eq!(k.text, "한글.txt");
        assert_eq!(k.encoding, NameEncoding::Utf8);
    }

    #[test]
    fn 変換できる名前はshift_jisになる() {
        let n = prepare_windows_name("日本語.txt");
        assert_eq!(n.encoding, NameEncoding::ShiftJis);
        let ascii = prepare_windows_name("abc.txt");
        assert_eq!(ascii.encoding, NameEncoding::ShiftJis);
        assert_eq!(ascii.bytes, b"abc.txt");
    }

    #[test]
    fn 円記号を含む名前は_utf8になり_バックスラッシュに化けない() {
        let n = prepare_windows_name("a¥b.txt");
        assert_eq!(n.text, "a¥b.txt");
        assert_eq!(n.encoding, NameEncoding::Utf8);
        assert!(!n.bytes.contains(&b'\\'));
        // 全角の円記号はShift_JISに変換できる
        assert_eq!(prepare_windows_name("a￥b.txt").encoding, NameEncoding::ShiftJis);
    }

    #[test]
    fn 半角カナとnec特殊文字とibm拡張はそのままshift_jisに変換できる() {
        let n = prepare_windows_name("ｱｲｳ①㈱髙.txt");
        assert_eq!(n.text, "ｱｲｳ①㈱髙.txt");
        assert_eq!(n.encoding, NameEncoding::ShiftJis);
        assert!(!n.replaced);
    }

    #[test]
    fn 波ダッシュを置き換えるとshift_jisで書ける() {
        // 〜を含むだけなら、全角チルダにして、名前全体がShift_JISになる(絵文字などが無ければ)。
        let n = prepare_windows_name("第1回〜第3回.txt");
        assert_eq!(n.encoding, NameEncoding::ShiftJis);
        assert!(n.replaced);
        // 〜と絵文字の両方を含む名前は、置き換えた上でUTF-8になる。
        let m = prepare_windows_name("〜😀.txt");
        assert_eq!(m.text, "～😀.txt");
        assert_eq!(m.encoding, NameEncoding::Utf8);
        assert!(m.replaced);
    }

    #[test]
    fn 判断はパス全体で行う() {
        // 1階層でも変換できない文字があれば、パス全体がUTF-8になる。
        let n = prepare_windows_name("日本語/😀/ファイル.txt");
        assert_eq!(n.encoding, NameEncoding::Utf8);
        assert_eq!(n.bytes, "日本語/😀/ファイル.txt".as_bytes());
    }

    // ---- 圧縮: 同名の連番 ----

    fn dedupe(paths: &[&str]) -> Vec<String> {
        dedupe_paths(&paths.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn 同名のファイルに連番を付ける() {
        assert_eq!(dedupe(&["a.txt", "a.txt", "a.txt"]), ["a.txt", "a_1.txt", "a_2.txt"]);
    }

    #[test]
    fn 階層の中の同名にも連番を付ける() {
        assert_eq!(dedupe(&["x/y/a.txt", "x/y/a.txt"]), ["x/y/a.txt", "x/y/a_1.txt"]);
        // 別の階層の同名ファイルは重複ではない
        assert_eq!(dedupe(&["x/a.txt", "y/a.txt"]), ["x/a.txt", "y/a.txt"]);
    }

    #[test]
    fn 大文字小文字だけ違う名前も同名として連番を付ける() {
        assert_eq!(dedupe(&["A.txt", "a.txt", "x/B/", "x/b/"]), ["A.txt", "a_1.txt", "x/B/", "x/b_1/"]);
    }

    #[test]
    fn 同名のディレクトリは末尾に連番を付ける() {
        assert_eq!(dedupe(&["d/", "d/", "v1.0/", "v1.0/"]), ["d/", "d_1/", "v1.0/", "v1.0_1/"]);
    }

    #[test]
    fn 連番が既存の名前と重なったらさらに進める() {
        assert_eq!(dedupe(&["a.txt", "a.txt", "a_1.txt"]), ["a.txt", "a_1.txt", "a_1_1.txt"]);
    }

    #[test]
    fn 重複がなければ変わらない() {
        let input = ["a.txt", "b.txt", "d/", "d/c.txt"];
        assert_eq!(dedupe(&input), input);
        assert!(dedupe(&[]).is_empty());
    }
}
