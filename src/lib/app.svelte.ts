// 画面の状態機械(docs/画面仕様.md 2章)。
//
// 状態: initial(ドロップエリアのみ) / compress(圧縮モード) / extract(解凍モード) / working(処理中)。
// 「完了」は、完了ダイアログとして表示し、閉じたら初期状態になる。

import * as api from "./api";
import { chooseSavePath, withZipExtension } from "./save";
import type {
  AppError,
  Charset,
  ConflictChoice,
  ConflictPolicy,
  EncryptionMethod,
  Item,
  PreviewRow,
  Progress,
  RenamedEntry,
  SkippedEntry,
  TargetOs,
  ZipInspection,
} from "./types";

export type Mode = "initial" | "compress" | "extract" | "working";

/** 解凍モードで表示・解凍に使うZipの情報。 */
export interface ExtractTarget {
  zipPath: string;
  zipName: string;
  inspection: ZipInspection;
}

/** パスワードを間違えてよい回数(この回数、間違えたら中止する)。 */
const MAX_PASSWORD_ATTEMPTS = 3;

/** 完了ダイアログの内容。 */
export interface DoneInfo {
  title: string;
  message: string;
  skipped: SkippedEntry[];
  renamed: RenamedEntry[];
}

/** 画面に出すダイアログ。`resolve` で、押されたボタンの結果を、呼び出し元に返す。 */
export type Dialog =
  | { kind: "conflict"; conflicts: string[]; resolve: (choice: ConflictChoice) => void }
  | { kind: "error"; message: string; resolve: () => void }
  | { kind: "confirm"; title: string; message: string; okLabel: string; resolve: (ok: boolean) => void }
  | { kind: "password"; title: string; message: string; resolve: (password: string | null) => void }
  | {
      kind: "charset";
      preview: PreviewRow[];
      /** どちらで読んでも、正しい文字コードとして読めない(どちらも化けるかもしれない)。 */
      neitherValid: boolean;
      resolve: (charset: Charset | null) => void;
    }
  | ({ kind: "done"; resolve: () => void } & DoneInfo);

class AppModel {
  mode = $state<Mode>("initial");
  /** 圧縮モードの一覧(ワークフォルダ直下)。 */
  items = $state<Item[]>([]);
  /** 解凍モードのZip。 */
  extractTarget = $state<ExtractTarget | null>(null);
  dialog = $state<Dialog | null>(null);
  /** ファイルをドラッグして、ウィンドウの上に来ている。 */
  dragging = $state(false);
  /** コマンド実行中(その間は、ドロップ・削除・クリアを受け付けない)。 */
  busy = $state(false);

  // 圧縮の設定(画面の入力欄)
  target = $state<TargetOs>("auto");
  method = $state<EncryptionMethod>("zipCrypto");
  password = $state("");

  // 処理中
  workLabel = $state("");
  progress = $state<Progress>({ done: 0, total: 0 });

  /** ドロップを受け付けられる状態か(解凍モードや処理中は、ほかのものを受け付けない)。 */
  get acceptsDrop(): boolean {
    return !this.busy && (this.mode === "initial" || this.mode === "compress");
  }

  /** 進み具合(0〜1)。全体の大きさが分からない(0バイト)ときは `null`。 */
  get fraction(): number | null {
    const { done, total } = this.progress;
    return total > 0 ? Math.min(done / total, 1) : null;
  }

  /** 起動時に、Rust側に残っている一覧を取り込む(開発中のリロードで、画面だけが初期化されたとき用)。 */
  async init() {
    try {
      this.setItems(await api.listItems());
    } catch {
      // 取得できなくても、初期状態で始められる
    }
  }

  // ---- ダイアログ ----

  askConflict(conflicts: string[]): Promise<ConflictChoice> {
    return new Promise((resolve) => {
      this.dialog = {
        kind: "conflict",
        conflicts,
        resolve: (choice) => {
          this.dialog = null;
          resolve(choice);
        },
      };
    });
  }

  showError(error: AppError | string): Promise<void> {
    const message = typeof error === "string" ? error : error.message;
    return new Promise((resolve) => {
      this.dialog = {
        kind: "error",
        message,
        resolve: () => {
          this.dialog = null;
          resolve();
        },
      };
    });
  }

  askConfirm(title: string, message: string, okLabel: string): Promise<boolean> {
    return new Promise((resolve) => {
      this.dialog = {
        kind: "confirm",
        title,
        message,
        okLabel,
        resolve: (ok) => {
          this.dialog = null;
          resolve(ok);
        },
      };
    });
  }

  /** パスワードの入力を求める。キャンセルなら `null`。 */
  askPassword(title: string, message: string): Promise<string | null> {
    return new Promise((resolve) => {
      this.dialog = {
        kind: "password",
        title,
        message,
        resolve: (password) => {
          this.dialog = null;
          resolve(password);
        },
      };
    });
  }

  /** ファイル名の文字コードを、読み比べプレビューで選ばせる。キャンセルなら `null`。 */
  askCharset(preview: PreviewRow[], neitherValid: boolean): Promise<Charset | null> {
    return new Promise((resolve) => {
      this.dialog = {
        kind: "charset",
        preview,
        neitherValid,
        resolve: (charset) => {
          this.dialog = null;
          resolve(charset);
        },
      };
    });
  }

  showDone(info: DoneInfo): Promise<void> {
    return new Promise((resolve) => {
      this.dialog = {
        kind: "done",
        ...info,
        resolve: () => {
          this.dialog = null;
          resolve();
        },
      };
    });
  }

  // ---- ドロップ ----

  /** ファイル・フォルダ・Zipがドロップされた。 */
  async handleDrop(paths: string[]) {
    if (!this.acceptsDrop || paths.length === 0) return;
    this.busy = true;
    try {
      const result = await api.classifyDrop(paths);
      if (result.kind === "extract") {
        this.extractTarget = {
          zipPath: result.zipPath,
          zipName: result.zipName,
          inspection: result.inspection,
        };
        this.mode = "extract";
        return;
      }

      // 同名があれば、1回だけ確認して、選んだ扱いを全部に適用する。
      let policy: ConflictPolicy = "rename";
      if (result.conflicts.length > 0) {
        const choice = await this.askConflict(result.conflicts);
        if (choice === "cancel") return;
        policy = choice;
      }
      this.setItems(await api.addItems(paths, policy));
    } catch (e) {
      await this.showError(api.toAppError(e));
    } finally {
      this.busy = false;
    }
  }

  /**
   * 「プログラムから開く」(Finder / エクスプローラー)で渡されたファイルを受け取って、
   * ドラッグ&ドロップされたときと同じように扱う。
   *
   * 起動時と、起動中に「開く要求が来た」通知を受けたときに呼ぶ。
   * ドロップと違って、ユーザには「受け付けられなかった」理由が見えないので、断るときは理由を表示する。
   */
  async handleOpenRequests() {
    let paths: string[];
    try {
      paths = await api.takePendingOpen();
    } catch {
      return;
    }
    if (paths.length === 0) return;

    // ダイアログを開いている最中は、そのやり取りを壊さないよう、何もしない(開いたものは受け付けない)。
    if (this.dialog !== null) return;
    if (this.busy || this.mode === "working") {
      await this.showError("処理中のため、開けませんでした。終わってから、もう一度開いてください。");
      return;
    }
    if (this.mode === "extract") {
      await this.showError("解凍待ちのZipがあるため、開けませんでした。「クリア」してから、もう一度開いてください。");
      return;
    }
    await this.handleDrop(paths);
  }

  // ---- 一覧の操作 ----

  /** 1件削除する。空になったら初期状態に戻る。 */
  async removeItem(name: string) {
    if (this.busy) return;
    this.busy = true;
    try {
      this.setItems(await api.removeItem(name));
    } catch (e) {
      await this.showError(api.toAppError(e));
    } finally {
      this.busy = false;
    }
  }

  /** クリア。初期状態に戻る。 */
  async clear() {
    if (this.busy) return;
    if (this.mode === "extract") {
      this.reset();
      return;
    }
    this.busy = true;
    try {
      await api.clearItems();
      this.reset();
    } catch (e) {
      await this.showError(api.toAppError(e));
    } finally {
      this.busy = false;
    }
  }

  // ---- 圧縮 ----

  /** 保存ボタン(画面仕様 3.2 の保存フロー)。 */
  async startCompress() {
    if (this.busy || this.mode !== "compress") return;
    this.busy = true;
    try {
      // 1. パスワードがあれば、確認のために再入力させる(一致しなければ中止。入力内容は残す)。
      const password = this.password;
      if (password !== "") {
        const again = await this.askPassword("パスワードの確認", "確認のため、もう一度パスワードを入力してください。");
        if (again === null) return;
        if (again !== password) {
          await this.showError("パスワードが一致しません。入力し直してください。");
          return;
        }
      }

      // 2. 保存先を選ばせる(同名があれば、OSのダイアログが上書きを確認する)。
      const chosen = await chooseSavePath(this.items);
      if (chosen === null) return;
      const dest = withZipExtension(chosen);
      // 拡張子を補ったときは、OSのダイアログの確認を通っていないので、こちらで確認する。
      if (dest !== chosen && (await api.pathExists(dest))) {
        const ok = await this.askConfirm("上書きの確認", `「${dest}」はすでにあります。置き換えますか?`, "置き換える");
        if (!ok) return;
      }

      // 3. 圧縮する。
      await this.runCompress(dest, password);
    } catch (e) {
      await this.showError(api.toAppError(e));
    } finally {
      this.busy = false;
    }
  }

  private async runCompress(dest: string, password: string) {
    this.startWork("圧縮中…");
    try {
      const encryption = password !== "" ? { method: this.method, password } : null;
      const report = await api.compress(dest, this.target, encryption, (p) => (this.progress = p));
      // 圧縮に成功すると、Rust側がワークフォルダを空にしている。初期状態に戻してから、完了を知らせる。
      this.reset();
      await this.showDone({
        title: "圧縮が完了しました",
        message: `保存しました: ${report.zipPath}`,
        skipped: report.skipped,
        renamed: report.renamed,
      });
    } catch (e) {
      const error = api.toAppError(e);
      if (error.kind === "cancelled") {
        // 中断したら、初期状態に戻る(画面仕様 3.4)。
        await this.discardItems();
        return;
      }
      // 失敗したら、一覧はそのままで、圧縮モードに戻る(やり直せる)。
      await this.restoreItems();
      await this.showError(error);
    }
  }

  // ---- 解凍 ----

  /** 解凍ボタン(画面仕様 3.3)。文字コードの確認 → パスワード入力(3回まで) → 解凍。 */
  async startExtract() {
    const target = this.extractTarget;
    if (this.busy || this.mode !== "extract" || !target) return;
    this.busy = true;
    try {
      // 1. 文字コードの判定が曖昧なときだけ、読み比べプレビューで選ばせる。
      const { detection, preview } = target.inspection;
      let charset: Charset | null = null;
      if (detection.kind === "ambiguous") {
        charset = await this.askCharset(preview, !detection.utf8Valid && !detection.sjisValid);
        if (charset === null) return;
      }

      // 2. パスワードが要るZipは、入力させる。間違えたら再入力(3回まで)。
      let needsPassword = target.inspection.encrypted;
      let password: string | null = null;
      let wrongCount = 0;
      for (;;) {
        if (needsPassword) {
          const message =
            wrongCount === 0
              ? "このZipはパスワード付きです。パスワードを入力してください。"
              : `パスワードが違います。もう一度入力してください(あと${MAX_PASSWORD_ATTEMPTS - wrongCount}回)。`;
          password = await this.askPassword("パスワードの入力", message);
          if (password === null) return;
        }

        // 3. 解凍する。
        const outcome = await this.runExtract(target, charset, password);
        if (outcome === "wrongPassword") {
          wrongCount += 1;
          if (wrongCount >= MAX_PASSWORD_ATTEMPTS) {
            await this.showError("パスワードが違います。3回間違えたので、解凍を中止しました。");
            return;
          }
        } else if (outcome === "passwordRequired") {
          needsPassword = true; // 判定では分からなかったが、パスワードが要るZipだった
        } else {
          return;
        }
      }
    } catch (e) {
      await this.showError(api.toAppError(e));
    } finally {
      this.busy = false;
    }
  }

  /** 解凍を1回実行する。パスワードの問題だけは、呼び出し元に返して、再入力させる。 */
  private async runExtract(
    target: ExtractTarget,
    charset: Charset | null,
    password: string | null,
  ): Promise<"finished" | "wrongPassword" | "passwordRequired"> {
    this.startWork("解凍中…");
    try {
      const report = await api.extract(target.zipPath, charset, password, (p) => (this.progress = p));
      this.reset();
      await this.showDone({
        title: "解凍が完了しました",
        message: `展開先: ${report.outDir}`,
        skipped: report.skipped,
        renamed: report.renamed,
      });
      return "finished";
    } catch (e) {
      const error = api.toAppError(e);
      if (error.kind === "cancelled") {
        // 中断したら、初期状態に戻る(画面仕様 3.4)。展開先は、Rust側が削除している。
        this.reset();
        return "finished";
      }
      // 失敗したら、解凍モードに戻る(Zipはそのまま。やり直せる)。
      this.mode = "extract";
      if (error.kind === "wrongPassword" || error.kind === "passwordRequired") return error.kind;
      await this.showError(error);
      return "finished";
    }
  }

  /** 処理中の中断(キャンセルボタン)。 */
  async cancelWork() {
    try {
      await api.cancel();
    } catch {
      // 中断の要求に失敗しても、処理が終われば画面は戻る
    }
  }

  private startWork(label: string) {
    this.workLabel = label;
    this.progress = { done: 0, total: 0 };
    this.mode = "working";
  }

  /** 中断後に、一覧を捨てて初期状態に戻る。 */
  private async discardItems() {
    try {
      await api.clearItems();
    } catch {
      // 残っても、次回の起動時に掃除される
    }
    this.reset();
  }

  /** 失敗後に、Rust側に残っている一覧を取り込んで、圧縮モードに戻る。 */
  private async restoreItems() {
    try {
      this.setItems(await api.listItems());
    } catch {
      this.reset();
    }
  }

  private setItems(items: Item[]) {
    this.items = items;
    this.mode = items.length > 0 ? "compress" : "initial";
  }

  /** 初期状態に戻す(圧縮の設定のうち、パスワードだけは残さない)。 */
  private reset() {
    this.items = [];
    this.extractTarget = null;
    this.password = "";
    this.mode = "initial";
  }
}

export const app = new AppModel();
