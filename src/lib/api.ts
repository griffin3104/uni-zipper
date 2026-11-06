// Rust側のTauriコマンドの呼び出し口。引数名は、Rust側の引数名(キャメルケース)に合わせる。

import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  AppError,
  CompressReport,
  ConflictPolicy,
  DropClassification,
  EncryptionArg,
  ExtractReport,
  Charset,
  Item,
  Progress,
  TargetOs,
} from "./types";

export function classifyDrop(paths: string[]): Promise<DropClassification> {
  return invoke("classify_drop", { paths });
}

export function addItems(paths: string[], policy: ConflictPolicy): Promise<Item[]> {
  return invoke("add_items", { paths, policy });
}

export function listItems(): Promise<Item[]> {
  return invoke("list_items");
}

export function removeItem(name: string): Promise<Item[]> {
  return invoke("remove_item", { name });
}

export function clearItems(): Promise<void> {
  return invoke("clear_items");
}

/** 進捗を受け取るChannelを作る。 */
function progressChannel(onProgress: (p: Progress) => void): Channel<Progress> {
  const channel = new Channel<Progress>();
  channel.onmessage = onProgress;
  return channel;
}

export function compress(
  dest: string,
  target: TargetOs,
  encryption: EncryptionArg | null,
  onProgress: (p: Progress) => void,
): Promise<CompressReport> {
  return invoke("compress", { dest, target, encryption, onProgress: progressChannel(onProgress) });
}

export function extract(
  zipPath: string,
  charset: Charset | null,
  password: string | null,
  onProgress: (p: Progress) => void,
): Promise<ExtractReport> {
  return invoke("extract", { zipPath, charset, password, onProgress: progressChannel(onProgress) });
}

export function cancel(): Promise<void> {
  return invoke("cancel");
}

/** コマンドの失敗(`{ kind, message }`)を `AppError` にそろえる。想定外の値は `io` 扱いにする。 */
export function toAppError(e: unknown): AppError {
  if (typeof e === "object" && e !== null && "kind" in e && "message" in e) {
    return e as AppError;
  }
  return { kind: "io", message: String(e) };
}
