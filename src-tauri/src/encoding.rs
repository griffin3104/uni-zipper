//! Zipのファイル名の文字コード判定と変換(docs/技術設計.md 5.1 / 5.2)。
//!
//! 対象はUTF-8とShift_JIS(CP932相当)の2つだけ。自動判定ライブラリは使わず、
//! 「バイト列がそのコードとして正しいか」で判定する。

#![allow(dead_code)] // 解凍・圧縮から使われるまでの暫定(実装ステップ③④)

use encoding_rs::SHIFT_JIS;
use serde::Serialize;

/// 判定対象の文字コード。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Charset {
    Utf8,
    ShiftJis,
}

/// Zipのエントリ名(生のバイト列)と、UTF-8フラグ(一般目的ビット11)。
#[derive(Debug, Clone, Copy)]
pub struct RawName<'a> {
    pub bytes: &'a [u8],
    pub utf8_flag: bool,
}

/// 文字コードの判定結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Detection {
    /// 判定が要らない(UTF-8フラグなしで、非ASCIIを含む名前が1つもない)。
    NotNeeded,
    /// 1つに決まった。
    Resolved { charset: Charset },
    /// 決められない。プレビューでユーザに選ばせる。
    /// どちらでも正しく読める(`both`)か、どちらでも読めない(`neither`)かは、フィールドで分かる。
    #[serde(rename_all = "camelCase")]
    Ambiguous { utf8_valid: bool, sjis_valid: bool },
}

/// プレビュー用の1行。同じ名前を、2通りの読み方で並べる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub as_utf8: String,
    pub as_shift_jis: String,
}

/// 全エントリ名から、文字コードを判定する。
///
/// - UTF-8フラグ付きのエントリは、UTF-8で確定しているので、判定に使わない。
/// - 残りの非ASCIIの名前が、**すべて**そのコードとして正しいときだけ、そのコードを候補とする。
/// - 候補が1つに絞れたら確定。
/// - 両方残った場合は、UTF-8として読んだ結果に日本語の文字が [`UTF8_CONFIDENT_CHARS`] 文字以上
///   あれば、UTF-8で確定する(下記)。それ以外は曖昧(プレビューで選ばせる)。
/// - 0個(どちらでも読めない)も曖昧。
/// - 全エントリで1つの文字コードを選ぶ(エントリごとの混在は考えない)。
///
/// # 両方で正しく読める場合の扱い
///
/// UTF-8の日本語は3バイト(`E3 83 95` など)で、Shift_JISの2バイトの組としても
/// 偶然成立することが多い(例: `ファイル.txt`)。そのままだと、Macで作ったZipの
/// カタカナ名が、毎回プレビュー確認になってしまう。
/// 一方、Shift_JISのバイト列が、偶然UTF-8の3バイト列として何文字も揃うことは、まず無い。
/// そのため、UTF-8として読んで日本語が複数文字出てくるなら、UTF-8と判断する。
///
/// # 既知の限界
///
/// Shift_JISの第2水準漢字(珍しい漢字。先頭バイトが `E0〜EA` など)だけが並ぶ名前は、
/// 偶然UTF-8の3バイト列に揃って、UTF-8と誤確定することがある。
/// 全漢字から無作為に作った20万件では約0.01%。日常的な文字(ひらがな・カタカナ・第1水準漢字)
/// は、先頭バイトが `81〜98` でUTF-8としては不正なので、混同しない(テストで確認している)。
/// 「かなを必須にする」案も試したが、`写真/001.jpg` のような漢字だけの名前が
/// 約4割で曖昧になり、実用的でないためやめた。
pub fn detect(names: &[RawName]) -> Detection {
    let mut targets = names
        .iter()
        .filter(|n| !n.utf8_flag && !n.bytes.is_ascii())
        .peekable();
    if targets.peek().is_none() {
        return Detection::NotNeeded;
    }

    let (mut utf8_valid, mut sjis_valid) = (true, true);
    let mut japanese_chars = 0;
    for name in targets {
        match std::str::from_utf8(name.bytes) {
            Ok(text) => japanese_chars += text.chars().filter(|&c| is_japanese_char(c)).count(),
            Err(_) => utf8_valid = false,
        }
        sjis_valid &= decode_shift_jis_strict(name.bytes).is_some();
    }

    match (utf8_valid, sjis_valid) {
        (true, false) => Detection::Resolved { charset: Charset::Utf8 },
        (false, true) => Detection::Resolved { charset: Charset::ShiftJis },
        (true, true) if japanese_chars >= UTF8_CONFIDENT_CHARS => {
            Detection::Resolved { charset: Charset::Utf8 }
        }
        (utf8_valid, sjis_valid) => Detection::Ambiguous { utf8_valid, sjis_valid },
    }
}

/// 両方で正しく読める場合に、UTF-8と判断するのに必要な、日本語の文字数(全エントリの合計)。
const UTF8_CONFIDENT_CHARS: usize = 2;

/// 日本語の文字か(ひらがな・カタカナ・漢字・CJK記号・全角/半角形)。
fn is_japanese_char(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{303F}'   // CJKの記号・句読点
        | '\u{3040}'..='\u{30FF}' // ひらがな・カタカナ
        | '\u{4E00}'..='\u{9FFF}' // CJK統合漢字
        | '\u{FF00}'..='\u{FFEF}' // 全角・半角形
    )
}

/// 選ばれた(または確定した)文字コードで、エントリ名を文字列にする。
///
/// UTF-8フラグ付き、またはASCIIだけの名前は、文字コードの選択によらずUTF-8として読む。
/// 変換できないバイトは `U+FFFD` になる(読めない名前でも展開は止めない)。
pub fn decode_name(name: &RawName, charset: Charset) -> String {
    if name.utf8_flag || name.bytes.is_ascii() {
        return String::from_utf8_lossy(name.bytes).into_owned();
    }
    decode_lossy(name.bytes, charset)
}

/// プレビュー用に、先頭から最大 `limit` 件の(重複しない)名前を、2通りで読んで返す。
///
/// 判定対象と同じく、UTF-8フラグなしで非ASCIIを含む名前だけを使う。
pub fn preview(names: &[RawName], limit: usize) -> Vec<PreviewRow> {
    let mut rows: Vec<PreviewRow> = Vec::new();
    for name in names.iter().filter(|n| !n.utf8_flag && !n.bytes.is_ascii()) {
        if rows.len() >= limit {
            break;
        }
        let row = PreviewRow {
            as_utf8: decode_lossy(name.bytes, Charset::Utf8),
            as_shift_jis: decode_lossy(name.bytes, Charset::ShiftJis),
        };
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    rows
}

/// 文字列をShift_JISのバイト列にする。1文字でも変換できなければ `None`。
///
/// `encoding_rs`(WHATWG仕様)は、円記号 `¥`(U+00A5)を `0x5C`(バックスラッシュ)、
/// オーバーライン `‾`(U+203E)を `0x7E`(チルダ)に変換してしまう。
/// ファイル名ではパス区切りに化けて危険なので、この2文字は変換不可として扱う。
pub fn encode_shift_jis_strict(s: &str) -> Option<Vec<u8>> {
    if s.contains(['\u{A5}', '\u{203E}']) {
        return None;
    }
    let (bytes, _, had_errors) = SHIFT_JIS.encode(s);
    if had_errors {
        None
    } else {
        Some(bytes.into_owned())
    }
}

/// 厳密なShift_JISデコード。不正なバイト列、または制御文字(C1)になるものは `None`。
fn decode_shift_jis_strict(bytes: &[u8]) -> Option<String> {
    let text = SHIFT_JIS.decode_without_bom_handling_and_without_replacement(bytes)?;
    // WHATWGのShift_JISは 0x80 などを制御文字(U+0080〜U+009F)に割り当てる。
    // ファイル名としては不自然なので、不正とみなす。
    if text.chars().any(|c| ('\u{80}'..='\u{9F}').contains(&c)) {
        return None;
    }
    Some(text.into_owned())
}

fn decode_lossy(bytes: &[u8], charset: Charset) -> String {
    match charset {
        Charset::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Charset::ShiftJis => SHIFT_JIS
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_normalization::UnicodeNormalization;

    fn sjis(s: &str) -> Vec<u8> {
        encode_shift_jis_strict(s).unwrap_or_else(|| panic!("Shift_JISにできない: {s}"))
    }

    fn nfd(s: &str) -> String {
        s.nfd().collect()
    }

    fn unflagged(bytes: &[u8]) -> RawName<'_> {
        RawName { bytes, utf8_flag: false }
    }

    /// 現実的なファイル名の例(短いもの、フォルダ付き、濁点あり、半角カナなし)。
    const SAMPLES: &[&str] = &[
        "あ.txt",
        "日本語.txt",
        "資料.pdf",
        "テスト.txt",
        "ファイル.txt",
        "写真/001.jpg",
        "議事録/2026年11月/第3回.docx",
        "がぎぐ.txt",
        "ぱぴぷ.txt",
        "報告書(最終版).xlsx",
    ];

    #[test]
    fn mac式_utf8でフラグなし_nfdは_utf8で確定する() {
        for sample in SAMPLES {
            let bytes = nfd(sample).into_bytes();
            assert_eq!(
                detect(&[unflagged(&bytes)]),
                Detection::Resolved { charset: Charset::Utf8 },
                "{sample}"
            );
        }
    }

    #[test]
    fn utf8でフラグなし_nfcも_utf8で確定する() {
        for sample in SAMPLES {
            let bytes = sample.as_bytes();
            assert_eq!(
                detect(&[unflagged(bytes)]),
                Detection::Resolved { charset: Charset::Utf8 },
                "{sample}"
            );
        }
    }

    #[test]
    fn windows式_shift_jisは_shift_jisで確定する() {
        for sample in SAMPLES {
            let bytes = sjis(sample);
            assert_eq!(
                detect(&[unflagged(&bytes)]),
                Detection::Resolved { charset: Charset::ShiftJis },
                "{sample}"
            );
        }
    }

    #[test]
    fn カタカナだけの名前は_両方で読めてもutf8で確定する() {
        // UTF-8の「ファイル」は、Shift_JISとしても(偶然)正しいバイト列になる
        let bytes = "ファイル.txt".as_bytes();
        assert!(decode_shift_jis_strict(bytes).is_some(), "前提: Shift_JISとしても読める");
        assert_eq!(
            detect(&[unflagged(bytes)]),
            Detection::Resolved { charset: Charset::Utf8 }
        );
    }

    #[test]
    fn 日本語が1文字だけで両方読める場合は曖昧のまま() {
        // 「ア」(E3 82 A2)は、Shift_JISでも「漢字 + 半角カナ」として読めてしまう。根拠が足りない
        let bytes = "ア".as_bytes();
        assert!(decode_shift_jis_strict(bytes).is_some(), "前提: Shift_JISとしても読める");
        assert_eq!(
            detect(&[unflagged(bytes)]),
            Detection::Ambiguous { utf8_valid: true, sjis_valid: true }
        );
    }

    #[test]
    fn 日本語の文字数は複数エントリの合計で数える() {
        // 1文字ずつの名前が2つ。合計2文字なので、UTF-8で確定する
        let a = "ア".as_bytes();
        let b = "イ".as_bytes();
        assert_eq!(
            detect(&[unflagged(a), unflagged(b)]),
            Detection::Resolved { charset: Charset::Utf8 }
        );
    }

    /// Shift_JISの第1水準漢字か(2バイト目までの値が 0x889F〜0x9872)。日常で使う漢字はほぼこの範囲。
    fn is_level1_kanji(c: char) -> bool {
        match encode_shift_jis_strict(&c.to_string()).as_deref() {
            Some(&[b0, b1]) => (0x889F..=0x9872).contains(&(u16::from(b0) << 8 | u16::from(b1))),
            _ => false,
        }
    }

    /// 現実的な文字: ひらがな・カタカナ・第1水準漢字。
    fn common_chars() -> Vec<char> {
        ('\u{3041}'..='\u{3096}')
            .chain('\u{30A1}'..='\u{30F6}')
            .chain('\u{4E00}'..='\u{9FFF}')
            .filter(|&c| {
                let kana = ('\u{3041}'..='\u{30F6}').contains(&c);
                (kana && encode_shift_jis_strict(&c.to_string()).is_some()) || is_level1_kanji(c)
            })
            .collect()
    }

    /// 意地悪な文字: Shift_JISで書ける、ひらがな・カタカナ・すべての漢字(第2水準を含む)。
    fn all_chars() -> Vec<char> {
        ('\u{3041}'..='\u{3096}')
            .chain('\u{30A1}'..='\u{30F6}')
            .chain('\u{4E00}'..='\u{9FFF}')
            .filter(|c| encode_shift_jis_strict(&c.to_string()).is_some())
            .collect()
    }

    /// 決まった順番で擬似乱数を出す(テストを再現できるようにする)。
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, bound: usize) -> usize {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % bound
        }
    }

    struct Tally {
        total: usize,
        /// UTF-8の名前を、Shift_JISと誤確定した例
        wrong_utf8: Vec<String>,
        /// Shift_JISの名前を、UTF-8と誤確定した例
        wrong_sjis: Vec<String>,
        utf8_ambiguous: usize,
        sjis_ambiguous: usize,
    }

    /// `chars` から2〜4文字の名前(半分は `.txt` 付き)を `total` 件作り、両方の文字コードで判定する。
    fn run_detection(chars: &[char], total: usize) -> Tally {
        let mut rng = Lcg(42);
        let mut t = Tally { total, wrong_utf8: vec![], wrong_sjis: vec![], utf8_ambiguous: 0, sjis_ambiguous: 0 };
        for i in 0..total {
            let len = 2 + rng.next(3);
            let mut name: String = (0..len).map(|_| chars[rng.next(chars.len())]).collect();
            if i % 2 == 0 {
                name.push_str(".txt");
            }
            match detect(&[unflagged(name.as_bytes())]) {
                Detection::Resolved { charset: Charset::ShiftJis } => t.wrong_utf8.push(name.clone()),
                Detection::Ambiguous { .. } => t.utf8_ambiguous += 1,
                _ => {}
            }
            let sj = sjis(&name);
            match detect(&[unflagged(&sj)]) {
                Detection::Resolved { charset: Charset::Utf8 } => t.wrong_sjis.push(name.clone()),
                Detection::Ambiguous { .. } => t.sjis_ambiguous += 1,
                _ => {}
            }
        }
        t
    }

    #[test]
    fn 現実的な文字の大量の組み合わせで_誤確定も曖昧も出ない() {
        let chars = common_chars();
        assert!(chars.len() > 2500, "第1水準漢字を含めて十分な文字数があること: {}", chars.len());
        let t = run_detection(&chars, 200_000);
        eprintln!(
            "[現実的] {}件: 曖昧(UTF-8側)={}, 曖昧(Shift_JIS側)={}, 誤確定(UTF-8→SJIS)={}, 誤確定(SJIS→UTF-8)={}",
            t.total, t.utf8_ambiguous, t.sjis_ambiguous, t.wrong_utf8.len(), t.wrong_sjis.len()
        );
        assert!(t.wrong_utf8.is_empty(), "例: {:?}", &t.wrong_utf8[..t.wrong_utf8.len().min(5)]);
        assert!(t.wrong_sjis.is_empty(), "例: {:?}", &t.wrong_sjis[..t.wrong_sjis.len().min(5)]);
        assert_eq!(t.utf8_ambiguous, 0);
        assert_eq!(t.sjis_ambiguous, 0);
    }

    #[test]
    fn 第2水準漢字を含む全文字でも_誤確定は極めて少ない() {
        // 既知の限界(detect の説明を参照)。珍しい漢字だけの名前では、ごくまれに誤確定する
        let chars = all_chars();
        assert!(chars.len() > 5000);
        let t = run_detection(&chars, 200_000);
        eprintln!(
            "[全文字] {}件: 曖昧(UTF-8側)={}, 曖昧(Shift_JIS側)={}, 誤確定(UTF-8→SJIS)={}, 誤確定(SJIS→UTF-8)={}",
            t.total, t.utf8_ambiguous, t.sjis_ambiguous, t.wrong_utf8.len(), t.wrong_sjis.len()
        );
        // UTF-8の名前を、Shift_JISと誤確定することは無い
        assert!(t.wrong_utf8.is_empty(), "例: {:?}", &t.wrong_utf8[..t.wrong_utf8.len().min(5)]);
        // Shift_JISの名前をUTF-8と誤確定するのは、0.1%未満
        assert!(t.wrong_sjis.len() * 1000 < t.total, "誤確定が多すぎる: {}", t.wrong_sjis.len());
        // 曖昧(プレビューになる)も、2%未満
        assert!(t.sjis_ambiguous * 50 < t.total, "Shift_JIS側の曖昧が多すぎる: {}", t.sjis_ambiguous);
        assert_eq!(t.utf8_ambiguous, 0);
    }

    #[test]
    fn 複数エントリは全体で1つに決まる() {
        // 1つ目は両方で読める短い名前でも、2つ目がShift_JISでしか読めなければ、全体がShift_JIS
        let a = sjis("ｱｲ.txt");
        let b = sjis("日本語.txt");
        assert_eq!(
            detect(&[unflagged(&a), unflagged(&b)]),
            Detection::Resolved { charset: Charset::ShiftJis }
        );
    }

    #[test]
    fn asciiだけなら判定は不要() {
        assert_eq!(detect(&[unflagged(b"readme.txt"), unflagged(b"a/b.txt")]), Detection::NotNeeded);
        assert_eq!(detect(&[]), Detection::NotNeeded);
    }

    #[test]
    fn utf8フラグ付きは判定に使わない() {
        // フラグ付きなら、Shift_JISでしか読めないバイト列があっても、判定対象にしない
        let bytes = sjis("日本語.txt");
        let flagged = RawName { bytes: &bytes, utf8_flag: true };
        assert_eq!(detect(&[flagged]), Detection::NotNeeded);

        // フラグなしの名前と混ざっていたら、フラグなしだけで判定する
        let utf8_bytes = "テスト.txt".as_bytes();
        assert_eq!(
            detect(&[flagged, unflagged(utf8_bytes)]),
            Detection::Resolved { charset: Charset::Utf8 }
        );
    }

    #[test]
    fn どちらでも読めない場合は曖昧になる() {
        // 0xFF 0xFE は、UTF-8としてもShift_JISとしても不正
        let bytes = [0xFF, 0xFE, b'.', b't'];
        assert_eq!(
            detect(&[unflagged(&bytes)]),
            Detection::Ambiguous { utf8_valid: false, sjis_valid: false }
        );
    }

    #[test]
    fn どちらでも読める場合は曖昧になる() {
        // 半角カナ「ﾃｱ」は、Shift_JISで C3 B1。UTF-8の `ñ` としても正しい
        let bytes = sjis("ﾃｱ.txt");
        assert_eq!(bytes[..2], [0xC3, 0xB1]);
        assert_eq!(
            detect(&[unflagged(&bytes)]),
            Detection::Ambiguous { utf8_valid: true, sjis_valid: true }
        );
    }

    #[test]
    fn 制御文字になるバイト列はshift_jisとして不正() {
        // 0x80 は、WHATWGのShift_JISでは U+0080(制御文字)になるが、名前としては不正扱い
        let bytes = [0x80, b'.', b't'];
        assert_eq!(
            detect(&[unflagged(&bytes)]),
            Detection::Ambiguous { utf8_valid: false, sjis_valid: false }
        );
    }

    #[test]
    fn 名前を選んだ文字コードで読める() {
        let utf8 = nfd("がぎ.txt").into_bytes();
        let sj = sjis("日本語.txt");
        assert_eq!(decode_name(&unflagged(&utf8), Charset::Utf8), nfd("がぎ.txt"));
        assert_eq!(decode_name(&unflagged(&sj), Charset::ShiftJis), "日本語.txt");
    }

    #[test]
    fn フラグ付きとasciiは文字コードの選択によらずutf8で読む() {
        let flagged_bytes = "日本語.txt".as_bytes();
        let flagged = RawName { bytes: flagged_bytes, utf8_flag: true };
        assert_eq!(decode_name(&flagged, Charset::ShiftJis), "日本語.txt");
        assert_eq!(decode_name(&unflagged(b"a/b.txt"), Charset::ShiftJis), "a/b.txt");
    }

    #[test]
    fn 読めない名前も展開は止めない() {
        let bytes = [0xFF, 0xFE, b'.', b't'];
        let text = decode_name(&unflagged(&bytes), Charset::Utf8);
        assert!(text.contains('\u{FFFD}'));
        assert!(text.ends_with(".t"));
    }

    #[test]
    fn プレビューは2通りの読み方を並べる() {
        let sj = sjis("日本語.txt");
        let rows = preview(&[unflagged(&sj)], 5);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].as_shift_jis, "日本語.txt");
        assert!(rows[0].as_utf8.contains('\u{FFFD}'));
    }

    #[test]
    fn プレビューは件数を制限し重複を除き_asciiとフラグ付きを含めない() {
        let a = sjis("あ.txt");
        let b = sjis("い.txt");
        let c = sjis("う.txt");
        let flagged = RawName { bytes: "え.txt".as_bytes(), utf8_flag: true };
        let names = [
            unflagged(b"ascii.txt"),
            flagged,
            unflagged(&a),
            unflagged(&a), // 重複
            unflagged(&b),
            unflagged(&c),
        ];
        let rows = preview(&names, 2);
        let sjis_view: Vec<_> = rows.iter().map(|r| r.as_shift_jis.as_str()).collect();
        assert_eq!(sjis_view, ["あ.txt", "い.txt"]);
        assert!(preview(&[unflagged(b"ascii.txt")], 5).is_empty());
    }

    #[test]
    fn shift_jisへの変換は_変換できない文字でnoneを返す() {
        assert!(encode_shift_jis_strict("日本語ABC").is_some());
        assert!(encode_shift_jis_strict("半角ｶﾅ").is_some());
        assert!(encode_shift_jis_strict("①㈱髙﨑").is_some());
        assert!(encode_shift_jis_strict("😀").is_none());
        assert!(encode_shift_jis_strict("한").is_none());
        assert!(encode_shift_jis_strict("〜").is_none()); // 波ダッシュ U+301C
        assert!(encode_shift_jis_strict("～").is_some()); // 全角チルダ U+FF5E
    }

    #[test]
    fn 円記号とオーバーラインはパス区切りに化けるので変換不可にする() {
        // encoding_rsは U+00A5 を 0x5C(バックスラッシュ)にしてしまう
        assert!(encode_shift_jis_strict("a¥b").is_none());
        assert!(encode_shift_jis_strict("a‾b").is_none());
        // 全角の円記号 U+FFE5 は、通常どおり2バイトの文字に変換できる
        assert_eq!(encode_shift_jis_strict("￥").unwrap().len(), 2);
    }
}
