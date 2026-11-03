//! 検証用プロトタイプ: Shift_JIS(CP932)のファイル名を、UTF-8フラグなしでZipに書けるか。
//!
//! 検証項目:
//!   1. `zip` クレートで、Shift_JISのバイト列を名前に指定して書けるか(UTF-8フラグが立たないか)
//!   2. ZipCrypto / AES-256 で暗号化して書き、同じクレートで復号できるか
//!   3. Shift_JISに変換できない文字は何か(エンコード可否の一覧)
//!
//! 使い方: `cargo run` (出力は ./out/ に保存される)

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use encoding_rs::SHIFT_JIS;
use unicode_normalization::UnicodeNormalization;
use zip::unstable::write::FileOptionsExt;
use zip::write::{FileOptions, SimpleFileOptions};
use zip::{AesMode, CompressionMethod, ZipArchive, ZipWriter};

#[derive(Clone, Copy, Debug)]
enum Enc {
    None,
    ZipCrypto,
    Aes256,
}

/// NFCに正規化してからShift_JISへ変換する。変換できない文字があれば None。
fn to_sjis(name: &str) -> Option<Vec<u8>> {
    let nfc: String = name.nfc().collect();
    let (bytes, _, had_errors) = SHIFT_JIS.encode(&nfc);
    if had_errors {
        None
    } else {
        Some(bytes.into_owned())
    }
}

fn write_zip(entries: &[(&str, &[u8])], enc: Enc, password: &str) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        let raw = to_sjis(name).unwrap_or_else(|| panic!("Shift_JISに変換できない: {name}"));
        let base: FileOptions<'_, '_, ()> =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let opts = match enc {
            Enc::None => base,
            Enc::ZipCrypto => base
                .with_deprecated_encryption(password.as_bytes())
                .expect("ZipCrypto"),
            Enc::Aes256 => base.with_aes_encryption(AesMode::Aes256, password),
        }
        .name(&raw);
        writer.start_file_with_options(opts).expect("start_file");
        writer.write_all(data).expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

/// 自前の最小パーサ: セントラルディレクトリを直接読み、(フラグ, 名前のバイト列)を返す。
/// `zip` クレートの読み取りに頼らず、実際に書かれたバイトを確認するために使う。
fn parse_central_directory(buf: &[u8]) -> Vec<(u16, Vec<u8>)> {
    let u16le = |o: usize| u16::from_le_bytes([buf[o], buf[o + 1]]);
    let u32le = |o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);

    // EOCD(0x06054b50)を末尾から探す
    let eocd = (0..=buf.len() - 22)
        .rev()
        .find(|&i| u32le(i) == 0x0605_4b50)
        .expect("EOCDが見つからない");
    let count = u16le(eocd + 10) as usize;
    let mut pos = u32le(eocd + 16) as usize;

    let mut out = Vec::new();
    for _ in 0..count {
        assert_eq!(u32le(pos), 0x0201_4b50, "セントラルヘッダのシグネチャ不正");
        let flags = u16le(pos + 8);
        let name_len = u16le(pos + 28) as usize;
        let extra_len = u16le(pos + 30) as usize;
        let comment_len = u16le(pos + 32) as usize;
        let name = buf[pos + 46..pos + 46 + name_len].to_vec();
        out.push((flags, name));
        pos += 46 + name_len + extra_len + comment_len;
    }
    out
}

/// 全エントリのUTF-8フラグ(bit11)を、ローカルヘッダとセントラルヘッダの両方で落とす。
///
/// 前提: すべてのエントリ名をShift_JISで書いている(= UTF-8として扱わせたくない)こと。
/// 制限: ローカルヘッダの位置が0xFFFFFFFF(Zip64)のエントリは扱わない(プロトタイプでは未対応)。
fn clear_utf8_flag(buf: &mut [u8]) {
    let u16le = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32le = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);

    let eocd = (0..=buf.len() - 22)
        .rev()
        .find(|&i| u32le(buf, i) == 0x0605_4b50)
        .expect("EOCDが見つからない");
    let count = u16le(buf, eocd + 10) as usize;
    let mut pos = u32le(buf, eocd + 16) as usize;

    for _ in 0..count {
        assert_eq!(u32le(buf, pos), 0x0201_4b50);
        let local = u32le(buf, pos + 42) as usize;
        assert_ne!(local, 0xFFFF_FFFF, "Zip64は未対応");
        assert_eq!(u32le(buf, local), 0x0403_4b50, "ローカルヘッダのシグネチャ不正");
        // 汎用ビットフラグ: セントラルは +8、ローカルは +6(下位バイトがbit0-7、上位バイトがbit8-15)
        buf[pos + 9] &= !0x08; // bit11 = 上位バイトのbit3
        buf[local + 7] &= !0x08;
        let name_len = u16le(buf, pos + 28) as usize;
        let extra_len = u16le(buf, pos + 30) as usize;
        let comment_len = u16le(buf, pos + 32) as usize;
        pos += 46 + name_len + extra_len + comment_len;
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn main() {
    let out_dir = Path::new("out");
    fs::create_dir_all(out_dir).expect("out作成");

    // ---- 検証1・2: 書き込み ----
    // 濁点は、わざとNFD(分解形)で渡して、NFC正規化も一緒に確かめる。
    let nfd_ga = "か\u{3099}き\u{3099}.txt"; // 「かき」の濁点が分離している形("がぎ.txt"のNFD)
    let entries: Vec<(&str, &[u8])> = vec![
        ("テスト.txt", "テスト".as_bytes()),
        ("日本語/ファイル.txt", "日本語".as_bytes()),
        (nfd_ga, "dakuten".as_bytes()),
        ("ｱｲｳ.txt", "half-width katakana".as_bytes()),
    ];

    let cases = [
        ("plain", Enc::None),
        ("zipcrypto", Enc::ZipCrypto),
        ("aes256", Enc::Aes256),
    ];
    let password = "pass1234";
    let mut all_ok = true;

    println!("=== 検証1: UTF-8フラグ(bit11)とファイル名のバイト列 ===");
    for (label, enc) in cases {
        let bytes = write_zip(&entries, enc, password);
        let path = out_dir.join(format!("{label}.zip"));
        fs::write(&path, &bytes).expect("zip保存");

        println!("\n[{label}] {} ({} bytes)", path.display(), bytes.len());
        for (i, (flags, raw)) in parse_central_directory(&bytes).iter().enumerate() {
            let utf8_flag = flags & (1 << 11) != 0;
            let encrypted = flags & 1 != 0;
            let expected = to_sjis(entries[i].0).unwrap();
            let (decoded, _, _) = SHIFT_JIS.decode(raw);
            let ok = !utf8_flag && *raw == expected;
            all_ok &= ok;
            println!(
                "  {} flags=0x{flags:04X} utf8フラグ={} 暗号化={}  名前bytes={}  → {decoded}",
                if ok { "OK " } else { "NG!" },
                utf8_flag,
                encrypted,
                hex(raw),
            );
        }

        // ---- 検証2: 復号(読み取り) ----
        let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("ZipArchive");
        for i in 0..archive.len() {
            let mut content = Vec::new();
            match enc {
                Enc::None => {
                    let mut f = archive.by_index(i).expect("by_index");
                    f.read_to_end(&mut content).expect("read");
                }
                _ => {
                    let mut f = archive
                        .by_index_decrypt(i, password.as_bytes())
                        .expect("by_index_decrypt");
                    f.read_to_end(&mut content).expect("read(decrypt)");
                }
            }
            let ok = content == entries[i].1;
            all_ok &= ok;
            println!(
                "  復号/読み取り {} : {}",
                if ok { "OK " } else { "NG!" },
                String::from_utf8_lossy(&content)
            );
        }
    }

    // ---- 検証3: Shift_JISに変換できる文字・できない文字 ----
    println!("\n=== 検証3: Shift_JIS(encoding_rs)でエンコードできるか ===");
    let probes: &[(&str, &str)] = &[
        ("ひらがな・カタカナ", "あいうアイウ"),
        ("全角チルダ U+FF5E", "～"),
        ("波ダッシュ U+301C(Macで出やすい)", "〜"),
        ("全角ハイフンマイナス U+FF0D", "－"),
        ("マイナス記号 U+2212", "−"),
        ("丸数字", "①"),
        ("株式会社の略号", "㈱"),
        ("はしごだか(IBM拡張)", "髙"),
        ("たつさき(IBM拡張)", "﨑"),
        ("ローマ数字", "Ⅲ"),
        ("単位記号", "㎏"),
        ("絵文字", "😀"),
        ("ハングル", "한"),
        ("半角カナ", "ｱ"),
    ];
    for (label, s) in probes {
        let r = to_sjis(s);
        println!(
            "  {} {:<28} {:<4} → {}",
            if r.is_some() { "OK " } else { "NG " },
            label,
            s,
            r.map(|b| hex(&b)).unwrap_or_else(|| "変換不可".to_string()),
        );
    }

    // ---- 検証4: Shift_JISのバイト列が、たまたまUTF-8として正しい場合 ----
    println!("\n=== 検証4: Shift_JISのバイト列がUTF-8として正しくなるケース ===");
    // `zip` クレートは「名前がUTF-8として正しく、非ASCII」ならUTF-8フラグを立てる実装。
    // Shift_JISのバイト列が偶然UTF-8として正しいと、フラグが誤って立つはず。
    let tricky: &[&str] = &["ﾃｱ.txt", "ﾁｱ.txt", "ﾂ.txt", "ｱ.txt"];
    for name in tricky {
        let raw = to_sjis(name).unwrap();
        let valid_utf8 = std::str::from_utf8(&raw).is_ok() && !raw.is_ascii();
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        let opts: FileOptions<'_, '_, ()> =
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        w.start_file_with_options(opts.name(&raw)).unwrap();
        w.write_all(b"x").unwrap();
        let bytes = w.finish().unwrap().into_inner();
        let (flags, _) = &parse_central_directory(&bytes)[0];
        let utf8_flag = flags & (1 << 11) != 0;
        println!(
            "  {:<10} bytes={:<12} UTF-8として正しい={:<5} → 書かれたUTF-8フラグ={}{}",
            name,
            hex(&raw),
            valid_utf8,
            utf8_flag,
            if valid_utf8 && utf8_flag { "  ⚠ 誤ってフラグが立った" } else { "" },
        );
    }

    // ---- 検証5: 後処理でUTF-8フラグを落とせるか ----
    println!("\n=== 検証5: 書き込み後にUTF-8フラグ(bit11)を落とす後処理 ===");
    for enc in [Enc::None, Enc::ZipCrypto, Enc::Aes256] {
        let tricky_entries: Vec<(&str, &[u8])> =
            vec![("ﾃｱ.txt", b"tricky"), ("テスト.txt", b"normal")];
        let mut bytes = write_zip(&tricky_entries, enc, password);
        let before: Vec<bool> = parse_central_directory(&bytes)
            .iter()
            .map(|(f, _)| f & (1 << 11) != 0)
            .collect();
        clear_utf8_flag(&mut bytes);
        let after: Vec<bool> = parse_central_directory(&bytes)
            .iter()
            .map(|(f, _)| f & (1 << 11) != 0)
            .collect();
        let path = out_dir.join(format!("tricky_{enc:?}.zip").to_lowercase());
        fs::write(&path, &bytes).expect("zip保存");

        // zipクレートで読み戻し、データが壊れていないことを確認する
        let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("ZipArchive");
        let mut all_read = true;
        for i in 0..archive.len() {
            let mut content = Vec::new();
            let res = match enc {
                Enc::None => archive
                    .by_index(i)
                    .map(|mut f| f.read_to_end(&mut content).map(|_| ())),
                _ => archive
                    .by_index_decrypt(i, password.as_bytes())
                    .map(|mut f| f.read_to_end(&mut content).map(|_| ())),
            };
            all_read &= matches!(res, Ok(Ok(()))) && content == tricky_entries[i].1;
        }
        let ok = after.iter().all(|f| !f) && all_read;
        all_ok &= ok;
        println!(
            "  {} {:?}: utf8フラグ 前={before:?} → 後={after:?} / 読み戻し={} ({})",
            if ok { "OK " } else { "NG!" },
            enc,
            if all_read { "OK" } else { "NG" },
            path.display(),
        );
    }

    println!(
        "\n総合: {}",
        if all_ok { "検証1・2はすべてOK" } else { "NGあり(上のログを確認)" }
    );
}
