# uni-zipper

## 概要
WindowsとMacOSでZipファイルをやり取りする場合、文字化けを起こすことが多々ある。
この問題を自動的にファイル名を修正する機能を持つZipの圧縮・解凍用のツールである。

## 対象環境
- 動作OS: Windows / macOS
- 対象言語: 日本語のファイル名のみ
- 対応形式: `.zip` のみ

## 機能概要

### 共通
- ドラッグ&ドロップで対象を指定する。
- 拡張子で圧縮か解凍かを自動判断する。
  - Zipを1つだけドロップ → 解凍
  - それ以外(複数Zip、Zipとの混在を含む) → 圧縮
- 「クリア」ボタンで起動直後の状態に戻せる。
- 濁点・半濁点の分離(NFD問題)に対処する。
- `.DS_Store`、`__MACOSX`、`Thumbs.db`、`desktop.ini` は自動で除外する。
- 処理中はプログレスバーを表示し、キャンセルできる。
- Finder / エクスプローラーの「プログラムから開く」で、Zipをこのアプリで開ける(ドロップしたのと同じ状態で起動する)。Windowsは、インストーラ(`-setup.exe`)で入れたときに、候補に登録される。

### 圧縮
- ドロップしたファイル・フォルダを一覧表示し、追加していける。
- 保存ボタンで、Zipの保存先とファイル名を指定する(初期位置はデスクトップ)。
- 圧縮先のOSを「自動」「Windows」から選べる。
  - 自動: UTF-8、NFC正規化。Windows / Mac / Linuxで開ける。
  - Windows: Shift_JIS(CP932)優先、NFC正規化。Shift_JISに変換できない名前だけUTF-8フラグ付き(7-Zip方式)。古いWindows標準機能・7-Zip・Lhaplus向け。
- パスワードを設定できる(ZipCrypto / AES-256)。

### 解凍
- 実行中のOSに合わせて、ファイル名を自動変換する。
- 文字コードの判定が曖昧なときは、Shift_JISとUTF-8の読み比べプレビューで選べる。
- Zipと同じ場所に、フォルダを自動作成して展開する(同名があれば `_1`、`_2` を付与)。
- パスワード付きZipに対応する(ZipCrypto / AES-256。パスワードの再入力は3回まで)。

詳細は [docs/機能要件.md](docs/機能要件.md) を参照。

## 使用技術
Tauriを使用して作成。

## ライセンス
[MIT License](LICENSE)

## 開発

### 必要なもの
- Node.js(v24で動作確認)
- Rust(`rustup` で導入。`cargo` / `rustc`)
- macOS: Xcode Command Line Tools
- Windows: MSVCビルドツール、WebView2

### コマンド
```
npm install          # 依存パッケージの導入
npm run tauri dev    # 開発起動
npm run check        # フロントエンドの型チェック
npm run tauri build  # 配布用ビルド(実行したOS向け)
cd src-tauri && cargo test   # Rustのテスト
```

- `npm run tauri dev`(開発用の実行)は、`.app` ではないので、macOSのシステムダイアログ(保存ダイアログの上書き確認など)が英語で出る。日本語の表示は、`npm run tauri build -- --bundles app` で作る `.app` で確認する。
- アイコンは、`python3 scripts/make-icon.py` で `assets/icon-1024.png` を作り、`npm run tauri icon assets/icon-1024.png` で各OS用に展開している。

### リリース

タグをpushすると、GitHub Actionsが、macOS(Apple Silicon / Intel)とWindowsのインストーラを作り、Releasesに載せる(`.github/workflows/release.yml`)。

```
git tag v0.9.0
git push origin v0.9.0
```

- タグは `v数字.数字.数字` の形だけ(WindowsのMSIが、`-rc1` のような版を受け付けないため)。
- アプリの版は、タグから自動で設定される(`tauri.conf.json` の版を、ビルド時に書き換える)。
- インストーラは、Releasesに直接アップロードする。ワークフォルダのArtifacts(保存期間つきの一時ファイル)は作らない。
- 署名はしていないので、初回の起動時に、macOSのGatekeeperやWindowsのSmartScreenの警告が出る(対処は、Releaseの説明に書いてある)。
- `main` へのpushとプルリクエストでは、macOSとWindowsで、型チェック・テスト・Lintを行う(`.github/workflows/ci.yml`)。

### ディレクトリ構成
- `src/` フロントエンド(Svelte + TypeScript、ビルドはVite。SvelteKitは使わない)
- `src-tauri/` バックエンド(Rust)
- `assets/` / `scripts/` アイコンの元画像と、それを作るスクリプト
- `.github/workflows/` GitHub Actions(CI、リリース)
- `docs/` 設計書(機能要件・画面仕様・技術設計)
- `prototypes/` 技術検証用のプロトタイプ(本体のビルドとは無関係)
