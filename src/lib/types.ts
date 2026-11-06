// Rust側(src-tauri/src)の型と対応する。JSONの形は docs/技術設計.md 7.1 を参照。

/** ワークフォルダ直下の1項目(圧縮モードの一覧の1行)。 */
export interface Item {
  name: string;
  isDir: boolean;
  isZip: boolean;
}

export type Charset = "utf8" | "shiftJis";

/** ファイル名の文字コードの判定結果。 */
export type Detection =
  | { kind: "notNeeded" }
  | { kind: "resolved"; charset: Charset }
  | { kind: "ambiguous"; utf8Valid: boolean; sjisValid: boolean };

/** 読み比べプレビューの1行。 */
export interface PreviewRow {
  asUtf8: string;
  asShiftJis: string;
}

export interface ZipInspection {
  entryCount: number;
  /** パスワードが要る。 */
  encrypted: boolean;
  detection: Detection;
  /** 判定が曖昧なときだけ入る。 */
  preview: PreviewRow[];
}

/** ドロップされたものの判定結果。 */
export type DropClassification =
  | { kind: "extract"; zipPath: string; zipName: string; inspection: ZipInspection }
  | { kind: "compress"; conflicts: string[] };

/** 同名があったときの扱い。 */
export type ConflictPolicy = "overwrite" | "rename";
/** 同名の確認ダイアログの選択。 */
export type ConflictChoice = ConflictPolicy | "cancel";

export type TargetOs = "auto" | "windows";
export type EncryptionMethod = "zipCrypto" | "aes256";

export interface EncryptionArg {
  method: EncryptionMethod;
  password: string;
}

export interface Progress {
  done: number;
  total: number;
}

export interface SkippedEntry {
  name: string;
  reason: string;
}

export interface RenamedEntry {
  from: string;
  to: string;
}

export interface CompressReport {
  zipPath: string;
  fileCount: number;
  skipped: SkippedEntry[];
  renamed: RenamedEntry[];
}

export interface ExtractReport {
  outDir: string;
  fileCount: number;
  skipped: SkippedEntry[];
  renamed: RenamedEntry[];
}

export type ErrorKind =
  | "io"
  | "symlinkRejected"
  | "invalidPath"
  | "itemNotFound"
  | "invalidZip"
  | "charsetNotResolved"
  | "passwordRequired"
  | "wrongPassword"
  | "cancelled"
  | "busy"
  | "emptyWorkDir";

/** コマンドが失敗したときに返ってくる値。`message` はそのまま表示できる。 */
export interface AppError {
  kind: ErrorKind;
  message: string;
}
