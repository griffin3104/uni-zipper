// 画面の状態機械(docs/画面仕様.md 2章)。
//
// 状態: initial(ドロップエリアのみ) / compress(圧縮モード) / extract(解凍モード)。
// 処理中・完了の状態は、圧縮・解凍の実装(⑤-3、⑤-4)で加える。

import * as api from "./api";
import type {
  AppError,
  ConflictChoice,
  ConflictPolicy,
  Item,
  ZipInspection,
} from "./types";

export type Mode = "initial" | "compress" | "extract";

/** 解凍モードで表示・解凍に使うZipの情報。 */
export interface ExtractTarget {
  zipPath: string;
  zipName: string;
  inspection: ZipInspection;
}

/** 画面に出すダイアログ。`resolve` で、押されたボタンの結果を、呼び出し元に返す。 */
export type Dialog =
  | { kind: "conflict"; conflicts: string[]; resolve: (choice: ConflictChoice) => void }
  | { kind: "error"; message: string; resolve: () => void };

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

  /** ドロップを受け付けられる状態か(解凍モードでは、ほかのものを受け付けない)。 */
  get acceptsDrop(): boolean {
    return !this.busy && this.mode !== "extract";
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

  private setItems(items: Item[]) {
    this.items = items;
    this.mode = items.length > 0 ? "compress" : "initial";
  }

  private reset() {
    this.items = [];
    this.extractTarget = null;
    this.mode = "initial";
  }
}

export const app = new AppModel();
