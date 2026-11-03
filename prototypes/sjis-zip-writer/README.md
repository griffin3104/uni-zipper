# sjis-zip-writer

uni-zipper の「Windows」モード(Shift_JIS名・UTF-8フラグなし)が、`zip` クレートで実現できるかを確認するプロトタイプ。
本体(`src-tauri`)のビルドとは無関係の、独立したCargoプロジェクト。

## 実行

```
cargo run
```

出力のZipは `out/` に保存される(Git管理外)。

## 検証内容

1. Shift_JISのバイト列を名前に指定して、UTF-8フラグ(bit11)なしで書けるか
2. ZipCrypto / AES-256 で書いて、同じクレートで復号できるか
3. Shift_JISに変換できない文字の一覧
4. Shift_JISのバイト列が、偶然UTF-8として正しい場合の挙動(フラグが誤って立つか)
5. 書き込み後にUTF-8フラグを落とす後処理が機能するか

## 結果

[docs/技術設計.md](../../docs/技術設計.md) の6.2を参照。

## 外部ツールでの確認(手動)

```
python3 -I -c "
import zipfile
with zipfile.ZipFile('out/plain.zip') as z:
    for i in z.infolist():
        print(bool(i.flag_bits & 0x800), i.filename.encode('cp437').decode('cp932'))
"
unzip -P pass1234 -t out/zipcrypto.zip
```
