// 保存先の指定(OSの保存ダイアログ)。初期位置はデスクトップ。

import { desktopDir, join } from "@tauri-apps/api/path";
import { save } from "@tauri-apps/plugin-dialog";
import type { Item } from "./types";

/** 保存ダイアログに最初に入れておくファイル名。1件ならその名前、複数なら「アーカイブ」。 */
export function defaultZipName(items: Item[]): string {
  if (items.length !== 1) return "アーカイブ.zip";
  const { name, isDir } = items[0];
  const dot = name.lastIndexOf(".");
  const stem = !isDir && dot > 0 ? name.slice(0, dot) : name;
  return `${stem}.zip`;
}

/** 保存先をOSのダイアログで選ばせる。キャンセルなら `null`。同名があれば、OSのダイアログが上書きを確認する。 */
export async function chooseSavePath(items: Item[]): Promise<string | null> {
  const defaultPath = await join(await desktopDir(), defaultZipName(items));
  return save({
    title: "Zipの保存先",
    defaultPath,
    filters: [{ name: "Zipファイル", extensions: ["zip"] }],
  });
}

/** 拡張子が `.zip` でなければ付ける。 */
export function withZipExtension(path: string): string {
  return /\.zip$/i.test(path) ? path : `${path}.zip`;
}
