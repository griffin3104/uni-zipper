//! 圧縮用のワークフォルダ(docs/技術設計.md 4章)。
//!
//! - ドロップされたファイル・フォルダを、アプリ専用の一時ディレクトリへコピーして保持する。
//! - シンボリックリンクは、ドロップ全体を拒否する。
//! - 同名の扱い(上書き / 自動連番リネーム)に対応する。
//! - 不要になったら削除する(`Drop`でも削除する)。起動時に古い残骸を掃除できる。


use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::{AppError, Result};

const APP_DIR_NAME: &str = "uni-zipper";
const RUN_DIR_PREFIX: &str = "run-";

/// ワークフォルダを置く、既定の親ディレクトリ(OSの一時ディレクトリ配下)。
pub fn default_base() -> PathBuf {
    std::env::temp_dir().join(APP_DIR_NAME)
}

/// 同名のものが既にあるときの扱い。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// 既存のものを削除して、置き換える。
    Overwrite,
    /// 連番を付けた別名で追加する(`file.txt` → `file_1.txt`)。
    Rename,
}

/// 画面の一覧に表示する、ワークフォルダ直下の1項目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub name: String,
    pub is_dir: bool,
    /// 拡張子が `.zip` のファイル(一覧で色を変えるため)。
    pub is_zip: bool,
}

pub struct WorkDir {
    root: PathBuf,
}

impl WorkDir {
    /// `base` の下に、実行ごとに一意なワークフォルダを作る。
    pub fn create(base: &Path) -> Result<Self> {
        fs::create_dir_all(base)?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = base.join(format!("{RUN_DIR_PREFIX}{}-{nanos}", std::process::id()));
        fs::create_dir(&root)?;
        Ok(Self { root })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// 前回の異常終了などで残った、古いワークフォルダを削除する。削除した数を返す。
    ///
    /// `max_age` より新しいものは、別のアプリ起動が使用中の可能性があるため残す。
    /// `keep` に指定したパスは、古くても削除しない。
    pub fn cleanup_stale(base: &Path, max_age: Duration, keep: Option<&Path>) -> usize {
        let Ok(entries) = fs::read_dir(base) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            let is_run_dir = entry.file_name().to_string_lossy().starts_with(RUN_DIR_PREFIX)
                && entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if !is_run_dir || keep == Some(path.as_path()) {
                continue;
            }
            let age = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .unwrap_or(Duration::MAX);
            if age >= max_age && fs::remove_dir_all(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// 渡されたパス(フォルダは中身も)に、シンボリックリンクが1つでもあればエラーにする。
    ///
    /// ドロップ全体を拒否するため、**コピーを始める前**に、全パスをこれで検査する。
    pub fn check_no_symlinks(paths: &[PathBuf]) -> Result<()> {
        for path in paths {
            let walker = WalkDir::new(path).follow_links(false).follow_root_links(false);
            for entry in walker {
                let entry = entry.map_err(|e| AppError::Io(e.into()))?;
                if entry.file_type().is_symlink() {
                    return Err(AppError::SymlinkRejected(entry.path().to_path_buf()));
                }
            }
        }
        Ok(())
    }

    /// ドロップされたパスから、ワークフォルダ直下での名前を取り出す。
    pub fn item_name(src: &Path) -> Result<String> {
        src.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| AppError::InvalidPath(src.to_path_buf()))
    }

    /// ワークフォルダ直下に、その名前のものが既にあるか。
    pub fn exists(&self, name: &str) -> bool {
        fs::symlink_metadata(self.root.join(name)).is_ok()
    }

    /// ファイルまたはフォルダ(再帰)をコピーして追加し、ワークフォルダ内での最終的な名前を返す。
    ///
    /// 同名が既にある場合は `policy` に従う。事前に [`WorkDir::exists`] で確認して、
    /// ユーザに選ばせてから呼ぶこと。途中で失敗したら、コピーしかけたものは削除する。
    pub fn add(&self, src: &Path, policy: ConflictPolicy) -> Result<String> {
        // 呼び出し側が検査済みでも、単体で呼ばれたときに安全なように、ここでも検査する。
        Self::check_no_symlinks(&[src.to_path_buf()])?;
        let src_meta = fs::symlink_metadata(src)?;
        let is_dir = src_meta.is_dir();

        let mut name = Self::item_name(src)?;
        if self.exists(&name) {
            match policy {
                ConflictPolicy::Overwrite => self.remove(&name)?,
                ConflictPolicy::Rename => name = self.unique_name(&name, is_dir),
            }
        }

        let dest = self.root.join(&name);
        let result = if is_dir {
            copy_dir_all(src, &dest)
        } else {
            fs::copy(src, &dest).map(|_| ()).map_err(AppError::from)
        };
        if let Err(e) = result {
            let _ = remove_path(&dest);
            return Err(e);
        }
        Ok(name)
    }

    /// 既存の名前と重ならない、連番付きの名前を作る。
    pub fn unique_name(&self, name: &str, is_dir: bool) -> String {
        (1u32..)
            .map(|n| numbered_name(name, n, is_dir))
            .find(|candidate| !self.exists(candidate))
            .expect("連番が尽きることはない")
    }

    /// ワークフォルダ直下の一覧(名前の昇順、大文字小文字は区別しない)。
    pub fn list(&self) -> Result<Vec<Item>> {
        let mut items = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type()?.is_dir();
            let is_zip = !is_dir && name.to_lowercase().ends_with(".zip");
            items.push(Item { name, is_dir, is_zip });
        }
        items.sort_by_key(|item| item.name.to_lowercase());
        Ok(items)
    }

    /// 直下の1項目を削除する。
    pub fn remove(&self, name: &str) -> Result<()> {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
            return Err(AppError::InvalidPath(PathBuf::from(name)));
        }
        let target = self.root.join(name);
        if fs::symlink_metadata(&target).is_err() {
            return Err(AppError::ItemNotFound(name.to_string()));
        }
        remove_path(&target)
    }

    /// 中身をすべて削除する(ワークフォルダ自体は残す)。
    pub fn clear(&self) -> Result<()> {
        for entry in fs::read_dir(&self.root)? {
            remove_path(&entry?.path())?;
        }
        Ok(())
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// 名前に連番を付ける。ファイルは拡張子の前に、フォルダは末尾に付ける。
///
/// - `file.txt` → `file_1.txt`
/// - `archive.tar.gz` → `archive.tar_1.gz`
/// - `folder` → `folder_1`、`folder.v1`(フォルダ) → `folder.v1_1`
/// - `.bashrc` → `.bashrc_1`(先頭のドットは拡張子とみなさない)
pub(crate) fn numbered_name(name: &str, n: u32, is_dir: bool) -> String {
    if !is_dir
        && let Some(dot) = name.rfind('.')
        && dot > 0
        && dot + 1 < name.len()
    {
        let (stem, ext) = name.split_at(dot);
        return format!("{stem}_{n}{ext}");
    }
    format!("{name}_{n}")
}

fn remove_path(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn copy_dir_all(src: &Path, dest: &Path) -> Result<()> {
    for entry in WalkDir::new(src).follow_links(false) {
        let entry = entry.map_err(|e| AppError::Io(e.into()))?;
        let relative = entry
            .path()
            .strip_prefix(src)
            .map_err(|_| AppError::InvalidPath(entry.path().to_path_buf()))?;
        let target = dest.join(relative);
        let file_type = entry.file_type();
        if file_type.is_dir() {
            fs::create_dir_all(&target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target)?;
        } else {
            // シンボリックリンクは事前に拒否済み。ソケットなどの特殊ファイルも扱わない。
            return Err(AppError::InvalidPath(entry.path().to_path_buf()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// 親ディレクトリ(base)と、その下のワークフォルダと、ドロップ元ディレクトリを用意する。
    fn setup() -> (TempDir, WorkDir, TempDir) {
        let base = TempDir::new().unwrap();
        let wd = WorkDir::create(base.path()).unwrap();
        let src = TempDir::new().unwrap();
        (base, wd, src)
    }

    #[test]
    fn 連番の名前を作れる() {
        assert_eq!(numbered_name("file.txt", 1, false), "file_1.txt");
        assert_eq!(numbered_name("file.txt", 12, false), "file_12.txt");
        assert_eq!(numbered_name("archive.tar.gz", 1, false), "archive.tar_1.gz");
        assert_eq!(numbered_name("README", 1, false), "README_1");
        assert_eq!(numbered_name(".bashrc", 1, false), ".bashrc_1");
        assert_eq!(numbered_name("end.", 1, false), "end._1");
        assert_eq!(numbered_name("folder", 1, true), "folder_1");
        assert_eq!(numbered_name("folder.v1", 2, true), "folder.v1_2");
        assert_eq!(numbered_name("日本語.txt", 1, false), "日本語_1.txt");
    }

    #[test]
    fn 実行ごとに別のワークフォルダができる() {
        let base = TempDir::new().unwrap();
        let a = WorkDir::create(base.path()).unwrap();
        let b = WorkDir::create(base.path()).unwrap();
        assert_ne!(a.path(), b.path());
        assert!(a.path().is_dir() && b.path().is_dir());
    }

    #[test]
    fn dropでワークフォルダが削除される() {
        let base = TempDir::new().unwrap();
        let wd = WorkDir::create(base.path()).unwrap();
        let path = wd.path().to_path_buf();
        drop(wd);
        assert!(!path.exists());
    }

    #[test]
    fn ファイルとフォルダを追加して一覧できる() {
        let (_base, wd, src) = setup();
        write(&src.path().join("b.txt"), "b");
        write(&src.path().join("dir/inner/x.txt"), "x");
        write(&src.path().join("A.zip"), "z");

        assert_eq!(wd.add(&src.path().join("b.txt"), ConflictPolicy::Rename).unwrap(), "b.txt");
        assert_eq!(wd.add(&src.path().join("dir"), ConflictPolicy::Rename).unwrap(), "dir");
        assert_eq!(wd.add(&src.path().join("A.zip"), ConflictPolicy::Rename).unwrap(), "A.zip");

        // フォルダは中身ごとコピーされる
        assert_eq!(read(&wd.path().join("dir/inner/x.txt")), "x");

        let items = wd.list().unwrap();
        let summary: Vec<_> = items.iter().map(|i| (i.name.as_str(), i.is_dir, i.is_zip)).collect();
        assert_eq!(summary, [("A.zip", false, true), ("b.txt", false, false), ("dir", true, false)]);
    }

    #[test]
    fn 日本語名も追加できる() {
        let (_base, wd, src) = setup();
        write(&src.path().join("日本語/ファイル.txt"), "あ");
        wd.add(&src.path().join("日本語"), ConflictPolicy::Rename).unwrap();
        assert_eq!(read(&wd.path().join("日本語/ファイル.txt")), "あ");
    }

    #[test]
    fn 同名は上書きできる() {
        let (_base, wd, src) = setup();
        write(&src.path().join("a/f.txt"), "old");
        write(&src.path().join("b/f.txt"), "new");
        wd.add(&src.path().join("a/f.txt"), ConflictPolicy::Overwrite).unwrap();

        assert!(wd.exists("f.txt"));
        let name = wd.add(&src.path().join("b/f.txt"), ConflictPolicy::Overwrite).unwrap();
        assert_eq!(name, "f.txt");
        assert_eq!(read(&wd.path().join("f.txt")), "new");
        assert_eq!(wd.list().unwrap().len(), 1);
    }

    #[test]
    fn フォルダの上書きは古い中身を残さない() {
        let (_base, wd, src) = setup();
        write(&src.path().join("a/d/old.txt"), "old");
        write(&src.path().join("b/d/new.txt"), "new");
        wd.add(&src.path().join("a/d"), ConflictPolicy::Overwrite).unwrap();
        wd.add(&src.path().join("b/d"), ConflictPolicy::Overwrite).unwrap();

        assert!(!wd.path().join("d/old.txt").exists());
        assert_eq!(read(&wd.path().join("d/new.txt")), "new");
    }

    #[test]
    fn 同名は連番でリネームして追加できる() {
        let (_base, wd, src) = setup();
        write(&src.path().join("a/f.txt"), "1");
        write(&src.path().join("b/f.txt"), "2");
        write(&src.path().join("c/f.txt"), "3");

        assert_eq!(wd.add(&src.path().join("a/f.txt"), ConflictPolicy::Rename).unwrap(), "f.txt");
        assert_eq!(wd.add(&src.path().join("b/f.txt"), ConflictPolicy::Rename).unwrap(), "f_1.txt");
        assert_eq!(wd.add(&src.path().join("c/f.txt"), ConflictPolicy::Rename).unwrap(), "f_2.txt");
        assert_eq!(read(&wd.path().join("f.txt")), "1");
        assert_eq!(read(&wd.path().join("f_1.txt")), "2");
        assert_eq!(read(&wd.path().join("f_2.txt")), "3");
    }

    #[test]
    fn 同名フォルダは拡張子扱いせず連番を付ける() {
        let (_base, wd, src) = setup();
        write(&src.path().join("a/v1.0/x.txt"), "1");
        write(&src.path().join("b/v1.0/x.txt"), "2");
        wd.add(&src.path().join("a/v1.0"), ConflictPolicy::Rename).unwrap();
        let name = wd.add(&src.path().join("b/v1.0"), ConflictPolicy::Rename).unwrap();
        assert_eq!(name, "v1.0_1");
    }

    #[test]
    fn 存在しないパスはエラーになり何も残らない() {
        let (_base, wd, src) = setup();
        let err = wd.add(&src.path().join("none.txt"), ConflictPolicy::Rename).unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
        assert!(wd.list().unwrap().is_empty());
    }

    #[test]
    fn 項目を削除できる() {
        let (_base, wd, src) = setup();
        write(&src.path().join("a.txt"), "a");
        write(&src.path().join("d/b.txt"), "b");
        wd.add(&src.path().join("a.txt"), ConflictPolicy::Rename).unwrap();
        wd.add(&src.path().join("d"), ConflictPolicy::Rename).unwrap();

        wd.remove("a.txt").unwrap();
        wd.remove("d").unwrap();
        assert!(wd.list().unwrap().is_empty());
        assert!(matches!(wd.remove("a.txt"), Err(AppError::ItemNotFound(_))));
    }

    #[test]
    fn 危険な名前の削除は拒否する() {
        let (base, wd, src) = setup();
        write(&base.path().join("outside.txt"), "keep");
        write(&src.path().join("a.txt"), "a");
        wd.add(&src.path().join("a.txt"), ConflictPolicy::Rename).unwrap();

        for name in ["", ".", "..", "../outside.txt", "a/b", "a\\b"] {
            assert!(matches!(wd.remove(name), Err(AppError::InvalidPath(_))), "{name}");
        }
        assert!(base.path().join("outside.txt").exists());
        assert!(wd.exists("a.txt"));
    }

    #[test]
    fn clearで中身だけ消える() {
        let (_base, wd, src) = setup();
        write(&src.path().join("a.txt"), "a");
        write(&src.path().join("d/b.txt"), "b");
        wd.add(&src.path().join("a.txt"), ConflictPolicy::Rename).unwrap();
        wd.add(&src.path().join("d"), ConflictPolicy::Rename).unwrap();

        wd.clear().unwrap();
        assert!(wd.list().unwrap().is_empty());
        assert!(wd.path().is_dir());
    }

    #[cfg(unix)]
    mod symlink {
        use super::*;
        use std::os::unix::fs::symlink;

        #[test]
        fn リンクそのものをドロップすると拒否する() {
            let (_base, wd, src) = setup();
            write(&src.path().join("real.txt"), "r");
            symlink(src.path().join("real.txt"), src.path().join("link.txt")).unwrap();

            let err = wd.add(&src.path().join("link.txt"), ConflictPolicy::Rename).unwrap_err();
            assert!(matches!(err, AppError::SymlinkRejected(_)));
            assert!(wd.list().unwrap().is_empty());
        }

        #[test]
        fn フォルダの中のリンクも拒否して何もコピーしない() {
            let (_base, wd, src) = setup();
            write(&src.path().join("dir/a.txt"), "a");
            write(&src.path().join("target.txt"), "t");
            symlink(src.path().join("target.txt"), src.path().join("dir/link.txt")).unwrap();

            let err = wd.add(&src.path().join("dir"), ConflictPolicy::Rename).unwrap_err();
            assert!(matches!(err, AppError::SymlinkRejected(_)));
            assert!(wd.list().unwrap().is_empty());
        }

        #[test]
        fn 壊れたリンクと_リンク先フォルダへのリンクも拒否する() {
            let (_base, _wd, src) = setup();
            write(&src.path().join("realdir/x.txt"), "x");
            symlink(src.path().join("realdir"), src.path().join("dirlink")).unwrap();
            symlink(src.path().join("nowhere"), src.path().join("broken")).unwrap();

            for name in ["dirlink", "broken"] {
                let err = WorkDir::check_no_symlinks(&[src.path().join(name)]).unwrap_err();
                assert!(matches!(err, AppError::SymlinkRejected(_)), "{name}");
            }
        }

        #[test]
        fn 複数ドロップのうち1つでもリンクがあれば全体を拒否する() {
            let (_base, _wd, src) = setup();
            write(&src.path().join("ok.txt"), "ok");
            write(&src.path().join("real.txt"), "r");
            symlink(src.path().join("real.txt"), src.path().join("link.txt")).unwrap();

            let paths = vec![src.path().join("ok.txt"), src.path().join("link.txt")];
            assert!(matches!(
                WorkDir::check_no_symlinks(&paths),
                Err(AppError::SymlinkRejected(_))
            ));
            assert!(WorkDir::check_no_symlinks(&paths[..1]).is_ok());
        }
    }

    #[test]
    fn 古いワークフォルダだけを掃除する() {
        let base = TempDir::new().unwrap();
        let current = WorkDir::create(base.path()).unwrap();
        let old = base.path().join("run-old-1");
        fs::create_dir(&old).unwrap();
        write(&old.join("x.txt"), "x");
        let unrelated = base.path().join("other-dir");
        fs::create_dir(&unrelated).unwrap();

        // max_age が十分に長ければ、何も消えない
        assert_eq!(WorkDir::cleanup_stale(base.path(), Duration::from_secs(3600), None), 0);
        assert!(old.exists());

        // max_age が 0 なら古いもの扱いになるが、keep と run- 以外は残る
        let removed = WorkDir::cleanup_stale(base.path(), Duration::ZERO, Some(current.path()));
        assert_eq!(removed, 1);
        assert!(!old.exists());
        assert!(current.path().exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn 親ディレクトリが無くても掃除はエラーにならない() {
        let base = TempDir::new().unwrap();
        let missing = base.path().join("nothing");
        assert_eq!(WorkDir::cleanup_stale(&missing, Duration::ZERO, None), 0);
    }
}
