//! テスト用のZip作成ヘルパー(圧縮の本実装 `zip_writer` ができるまでの橋渡しも兼ねる)。
//!
//! 名前をバイト列で直接指定できるので、Shift_JISや、UTF-8フラグなしのUTF-8(Macのアーカイブユーティリティ相当)
//! など、実機で出会うZipを再現できる。

use std::io::{Cursor, Write};

use zip::unstable::write::FileOptionsExt;
use zip::write::{FileOptions, SimpleFileOptions};
use zip::{AesMode, CompressionMethod, ZipWriter};

/// 暗号化の指定。
#[derive(Debug, Clone, Copy)]
pub enum Enc<'a> {
    None,
    ZipCrypto(&'a str),
    Aes256(&'a str),
}

/// (名前のバイト列, 中身)。名前が `/` で終わるものはディレクトリ。
pub type Entry<'a> = (&'a [u8], &'a [u8]);

/// Zipを作る。UTF-8として正しい非ASCIIの名前には、`zip` クレートが自動でUTF-8フラグを立てる。
pub fn build_zip(entries: &[Entry], enc: Enc) -> Vec<u8> {
    build(entries, enc, CompressionMethod::Deflated)
}

/// 圧縮しないで格納したZipを作る(中身がそのまま見えるので、壊すテストに使う)。
pub fn build_zip_stored(entries: &[Entry]) -> Vec<u8> {
    build(entries, Enc::None, CompressionMethod::Stored)
}

fn build(entries: &[Entry], enc: Enc, method: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        let base: FileOptions<'_, '_, ()> = SimpleFileOptions::default().compression_method(method);
        let opts = match enc {
            Enc::None => base,
            Enc::ZipCrypto(pw) => base.with_deprecated_encryption(pw.as_bytes()).expect("ZipCrypto"),
            Enc::Aes256(pw) => base.with_aes_encryption(AesMode::Aes256, pw),
        }
        .name(name);
        writer.start_file_with_options(opts).expect("start_file");
        writer.write_all(data).expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

/// シンボリックリンクのエントリを1つ含むZipを作る。
pub fn build_zip_with_symlink(link_name: &str, target: &str) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink(link_name, target, SimpleFileOptions::default())
        .expect("add_symlink");
    writer.start_file("ok.txt", SimpleFileOptions::default()).expect("start_file");
    writer.write_all(b"ok").expect("write");
    writer.finish().expect("finish").into_inner()
}

/// 全エントリのUTF-8フラグ(ビット11)を、ローカル・セントラル両方のヘッダで落とす。
///
/// Macのアーカイブユーティリティが作るZip(UTF-8の名前だがフラグなし)を再現するために使う。
/// Zip64(ローカルヘッダの位置が `0xFFFFFFFF`)は扱わない。
pub fn clear_utf8_flag(buf: &mut [u8]) {
    let u16le = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32le = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);

    let eocd = (0..=buf.len() - 22)
        .rev()
        .find(|&i| u32le(buf, i) == 0x0605_4b50)
        .expect("EOCDが見つからない");
    let count = u16le(buf, eocd + 10) as usize;
    let mut pos = u32le(buf, eocd + 16) as usize;

    for _ in 0..count {
        assert_eq!(u32le(buf, pos), 0x0201_4b50, "セントラルヘッダのシグネチャ不正");
        let local = u32le(buf, pos + 42) as usize;
        assert_eq!(u32le(buf, local), 0x0403_4b50, "ローカルヘッダのシグネチャ不正");
        buf[pos + 9] &= !0x08; // ビット11 = 汎用フラグ上位バイトのビット3
        buf[local + 7] &= !0x08;
        let name_len = u16le(buf, pos + 28) as usize;
        let extra_len = u16le(buf, pos + 30) as usize;
        let comment_len = u16le(buf, pos + 32) as usize;
        pos += 46 + name_len + extra_len + comment_len;
    }
}

/// Unicodeパス拡張フィールド(ID `0x7075`)を、未知のID(`0x7076`)に書き換えて無効にする。
///
/// 7-Zipなどは、Shift_JISの名前と一緒にUTF-8の名前も `0x7075` で持たせる。`zip` クレートはこれを
/// 解釈して名前を置き換えるので、Shift_JISの名前そのものの判定を試すときは、これで無効にする。
pub fn disable_unicode_path_field(buf: &mut [u8]) {
    let u16le = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32le = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);

    fn patch(buf: &mut [u8], mut at: usize, end: usize) {
        while at + 4 <= end {
            let id = u16::from_le_bytes([buf[at], buf[at + 1]]);
            let len = u16::from_le_bytes([buf[at + 2], buf[at + 3]]) as usize;
            if id == 0x7075 {
                buf[at..at + 2].copy_from_slice(&0x7076u16.to_le_bytes());
            }
            at += 4 + len;
        }
    }

    let eocd = (0..=buf.len() - 22)
        .rev()
        .find(|&i| u32le(buf, i) == 0x0605_4b50)
        .expect("EOCDが見つからない");
    let count = u16le(buf, eocd + 10) as usize;
    let mut pos = u32le(buf, eocd + 16) as usize;

    for _ in 0..count {
        let name_len = u16le(buf, pos + 28) as usize;
        let extra_len = u16le(buf, pos + 30) as usize;
        let comment_len = u16le(buf, pos + 32) as usize;
        patch(buf, pos + 46 + name_len, pos + 46 + name_len + extra_len);

        let local = u32le(buf, pos + 42) as usize;
        let (lname, lextra) = (u16le(buf, local + 26) as usize, u16le(buf, local + 28) as usize);
        patch(buf, local + 30 + lname, local + 30 + lname + lextra);

        pos += 46 + name_len + extra_len + comment_len;
    }
}
