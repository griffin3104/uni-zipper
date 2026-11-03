# uni-zipper

WindowsとmacOSの間でやり取りするZipの、ファイル名の文字化け(Shift_JIS/UTF-8、NFD問題)を自動で直す、Zipの圧縮・解凍ツール。Tauri 2 + Svelte 5 + TypeScript(Vite)。

## 作業ルール

- **コミット・プッシュはしない。** `git commit` / `git push` は、ユーザ(フィン)がやる。
  - 作業のひと区切りごとに、変更内容と確認結果を報告して止まる。コミットはユーザの合図を待つ。
  - ユーザから明示的に頼まれた場合だけ、コミットする。
- 設計書が仕様の正。仕様を変える・決めたときは、実装と一緒に `docs/` も更新する。
  - `docs/機能要件.md` 要件
  - `docs/画面仕様.md` 画面と操作
  - `docs/技術設計.md` 構成・設計判断・検証結果・未決事項
- ドキュメント、コメント、UIの文言は日本語。

## 開発メモ

- Bashツールのシェルには、`~/.cargo/bin` がPATHに入っていない。cargoを使うときは、先に `export PATH="$HOME/.cargo/bin:$PATH"` を実行する。
- Rustのテスト: `cd src-tauri && cargo test`
- フロントエンドの型チェック: `npm run check`
- `npm run tauri dev` を起動確認で使うときは、終了後に `vite` と `target/debug/uni-zipper` のプロセスが残っていないか確認して、残っていれば止める。
- `prototypes/` 配下は本体とは独立したCargoプロジェクト。本体のビルドには含めない。
- Windows実機が無いため、Windows標準機能での表示確認は、ユーザが用意したテスト用Zipか、GitHub ActionsのWindows環境で行う。
