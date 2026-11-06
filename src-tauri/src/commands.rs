//! フロントエンドから呼ぶコマンド(docs/技術設計.md 7章)。
//!
//! 処理の本体は [`AppState`] のメソッドに置き(テストしやすいように、Tauriに依存させない)、
//! `#[tauri::command]` の関数は、それを別スレッドで呼ぶだけの薄い包みにする。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::Channel;

use crate::encoding::Charset;
use crate::error::{AppError, Result};
use crate::normalize::Platform;
use crate::progress::{CancelToken, Progress};
use crate::workdir::{ConflictPolicy, Item, WorkDir};
use crate::zip_reader::{self, ExtractOptions, ExtractReport, ZipInspection};
use crate::zip_writer::{self, CompressOptions, CompressReport, EncryptionMethod, TargetOs};

/// 前回の異常終了などで残ったワークフォルダを、起動時に削除する基準(これより古いもの)。
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

// ---------------------------------------------------------------------------
// 状態
// ---------------------------------------------------------------------------

/// アプリ全体の状態。Tauriの管理状態として登録し、コマンドから共有する。
///
/// 持つのは、ワークフォルダ、「処理中」フラグ、処理中のキャンセルフラグの3つだけ。
/// 画面の状態(初期 / 圧縮 / 解凍 / …)は、フロントエンドが持つ。
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    workdir: WorkDir,
    busy: AtomicBool,
    cancel: Mutex<Option<CancelToken>>,
}

/// 処理中の間だけ保持する。破棄すると、「処理中」が解除される。
struct BusyGuard {
    inner: Arc<Inner>,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        *self.inner.cancel.lock().unwrap() = None;
        self.inner.busy.store(false, Ordering::SeqCst);
    }
}

impl AppState {
    /// `base` の下にワークフォルダを作り、古い残骸を掃除する。
    pub fn new(base: &Path) -> Result<Self> {
        let workdir = WorkDir::create(base)?;
        WorkDir::cleanup_stale(base, STALE_AFTER, Some(workdir.path()));
        Ok(Self {
            inner: Arc::new(Inner {
                workdir,
                busy: AtomicBool::new(false),
                cancel: Mutex::new(None),
            }),
        })
    }

    /// 処理を始める。すでに処理中なら `Busy`。戻り値のキャンセルフラグは、`cancel` で立てられる。
    fn begin(&self) -> Result<(BusyGuard, CancelToken)> {
        if self
            .inner
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(AppError::Busy);
        }
        let token = CancelToken::new();
        *self.inner.cancel.lock().unwrap() = Some(token.clone());
        Ok((BusyGuard { inner: self.inner.clone() }, token))
    }

    /// 処理中のキャンセル。処理中でなければ、何もしない。
    pub fn cancel(&self) {
        if let Some(token) = self.inner.cancel.lock().unwrap().as_ref() {
            token.cancel();
        }
    }

    // ---- ドロップ ----

    /// ドロップされたものを判定する(まだ何も追加しない)。
    ///
    /// - 一覧が空で、`.zip` のファイル1つだけ → 解凍モード。Zipの中身を調べて返す。
    /// - それ以外 → 圧縮モード。シンボリックリンクがあれば、ドロップ全体を拒否する。
    ///   すでにある名前(またはドロップの中で重なる名前)を `conflicts` で返す。
    pub fn classify_drop(&self, paths: &[PathBuf]) -> Result<DropClassification> {
        let (_guard, _) = self.begin()?;
        let workdir = &self.inner.workdir;

        if let [path] = paths
            && is_zip_file(path)
            && workdir.list()?.is_empty()
        {
            let inspection = zip_reader::inspect(path)?;
            return Ok(DropClassification::Extract {
                zip_path: path.to_string_lossy().into_owned(),
                zip_name: WorkDir::item_name(path)?,
                inspection,
            });
        }

        WorkDir::check_no_symlinks(paths)?;
        let mut conflicts: Vec<String> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for path in paths {
            let name = WorkDir::item_name(path)?;
            let duplicated = seen.iter().any(|s| s.eq_ignore_ascii_case(&name));
            if (workdir.exists(&name) || duplicated) && !conflicts.contains(&name) {
                conflicts.push(name.clone());
            }
            seen.push(name);
        }
        Ok(DropClassification::Compress { conflicts })
    }

    /// ワークフォルダに追加して、追加後の一覧を返す。同名は `policy` に従う(全部に同じ扱い)。
    ///
    /// 途中で失敗したら、この呼び出しで追加した分を取り消す(一部だけ追加された状態にしない)。
    pub fn add_items(&self, paths: &[PathBuf], policy: ConflictPolicy) -> Result<Vec<Item>> {
        let (_guard, _) = self.begin()?;
        let workdir = &self.inner.workdir;
        WorkDir::check_no_symlinks(paths)?;

        let mut added: Vec<String> = Vec::new();
        for path in paths {
            match workdir.add(path, policy) {
                Ok(name) => added.push(name),
                Err(e) => {
                    for name in &added {
                        let _ = workdir.remove(name);
                    }
                    return Err(e);
                }
            }
        }
        workdir.list()
    }

    pub fn list_items(&self) -> Result<Vec<Item>> {
        self.inner.workdir.list()
    }

    /// 1件削除して、削除後の一覧を返す。
    pub fn remove_item(&self, name: &str) -> Result<Vec<Item>> {
        let (_guard, _) = self.begin()?;
        self.inner.workdir.remove(name)?;
        self.inner.workdir.list()
    }

    /// 全件削除する。
    pub fn clear(&self) -> Result<()> {
        let (_guard, _) = self.begin()?;
        self.inner.workdir.clear()
    }

    // ---- 圧縮・解凍 ----

    /// ワークフォルダの中身を、`dest` にZipとして保存する。成功したら、ワークフォルダを空にする。
    /// 失敗・キャンセルのときは、ワークフォルダはそのまま残す(やり直せる)。
    pub fn compress(
        &self,
        dest: &Path,
        target: TargetOs,
        encryption: Option<EncryptionArg>,
        on_progress: &mut dyn FnMut(Progress),
    ) -> Result<CompressReport> {
        let (_guard, cancel) = self.begin()?;
        let workdir = &self.inner.workdir;
        if workdir.list()?.is_empty() {
            return Err(AppError::EmptyWorkDir);
        }
        let options = CompressOptions {
            target,
            encryption: encryption.as_ref().map(|e| (e.method, e.password.as_str())),
            cancel: &cancel,
        };
        let report = zip_writer::compress(workdir.path(), dest, &options, on_progress)?;
        // Zipは保存できているので、片付けの失敗は無視する(次回の起動時にも掃除される)。
        let _ = workdir.clear();
        Ok(report)
    }

    /// Zipを、同じ場所の自動のフォルダに展開する。
    pub fn extract(
        &self,
        zip: &Path,
        charset: Option<Charset>,
        password: Option<String>,
        on_progress: &mut dyn FnMut(Progress),
    ) -> Result<ExtractReport> {
        let (_guard, cancel) = self.begin()?;
        let options = ExtractOptions {
            charset,
            password: password.as_deref(),
            platform: Platform::current(),
            cancel: &cancel,
        };
        zip_reader::extract(zip, &options, on_progress)
    }
}

/// `.zip` の通常ファイルか(拡張子の大文字小文字は問わない)。
fn is_zip_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

// ---------------------------------------------------------------------------
// フロントエンドとやり取りする型
// ---------------------------------------------------------------------------

/// `classify_drop` の結果。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DropClassification {
    /// 解凍モードにする。
    #[serde(rename_all = "camelCase")]
    Extract {
        zip_path: String,
        zip_name: String,
        inspection: ZipInspection,
    },
    /// 圧縮モードにする。`conflicts` が空でなければ、追加の前にユーザに確認する。
    #[serde(rename_all = "camelCase")]
    Compress { conflicts: Vec<String> },
}

/// 暗号化の指定。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionArg {
    pub method: EncryptionMethod,
    pub password: String,
}

// ---------------------------------------------------------------------------
// Tauriコマンド(薄い包み)
// ---------------------------------------------------------------------------

fn to_paths(paths: Vec<String>) -> Vec<PathBuf> {
    paths.into_iter().map(PathBuf::from).collect()
}

/// 重い処理を、UIを止めないよう別スレッドで実行する。
async fn run_blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Io(std::io::Error::other(e.to_string())))?
}

#[tauri::command]
pub async fn classify_drop(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<DropClassification> {
    let state = state.inner().clone();
    run_blocking(move || state.classify_drop(&to_paths(paths))).await
}

#[tauri::command]
pub async fn add_items(
    state: State<'_, AppState>,
    paths: Vec<String>,
    policy: ConflictPolicy,
) -> Result<Vec<Item>> {
    let state = state.inner().clone();
    run_blocking(move || state.add_items(&to_paths(paths), policy)).await
}

#[tauri::command]
pub async fn list_items(state: State<'_, AppState>) -> Result<Vec<Item>> {
    state.list_items()
}

#[tauri::command]
pub async fn remove_item(state: State<'_, AppState>, name: String) -> Result<Vec<Item>> {
    let state = state.inner().clone();
    run_blocking(move || state.remove_item(&name)).await
}

#[tauri::command]
pub async fn clear_items(state: State<'_, AppState>) -> Result<()> {
    let state = state.inner().clone();
    run_blocking(move || state.clear()).await
}

#[tauri::command]
pub async fn compress(
    state: State<'_, AppState>,
    dest: String,
    target: TargetOs,
    encryption: Option<EncryptionArg>,
    on_progress: Channel<Progress>,
) -> Result<CompressReport> {
    let state = state.inner().clone();
    run_blocking(move || {
        state.compress(Path::new(&dest), target, encryption, &mut |p| {
            let _ = on_progress.send(p);
        })
    })
    .await
}

#[tauri::command]
pub async fn extract(
    state: State<'_, AppState>,
    zip_path: String,
    charset: Option<Charset>,
    password: Option<String>,
    on_progress: Channel<Progress>,
) -> Result<ExtractReport> {
    let state = state.inner().clone();
    run_blocking(move || {
        state.extract(Path::new(&zip_path), charset, password, &mut |p| {
            let _ = on_progress.send(p);
        })
    })
    .await
}

#[tauri::command]
pub fn cancel(state: State<'_, AppState>) {
    state.cancel();
}

/// そのパスに、ファイルまたはフォルダがあるか。保存先の上書き確認(OSの保存ダイアログを通らない場合)に使う。
#[tauri::command]
pub fn path_exists(path: String) -> bool {
    Path::new(&path).exists()
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::Detection;
    use crate::test_util::{Enc, build_zip};
    use std::fs;
    use tempfile::TempDir;

    /// 状態と、ドロップ元のファイルを置く一時フォルダを用意する。
    fn setup() -> (AppState, TempDir, TempDir) {
        let base = TempDir::new().unwrap();
        let src = TempDir::new().unwrap();
        let state = AppState::new(base.path()).unwrap();
        (state, base, src)
    }

    fn file(src: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = src.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn zip_file(src: &TempDir, name: &str, entries: &[(&[u8], &[u8])], enc: Enc) -> PathBuf {
        let path = src.path().join(name);
        fs::write(&path, build_zip(entries, enc)).unwrap();
        path
    }

    fn names(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.name.as_str()).collect()
    }

    fn no_progress() -> impl FnMut(Progress) {
        |_| {}
    }

    // ---- classify_drop ----

    #[test]
    fn zip1つだけをドロップすると解凍モードになる() {
        let (state, _b, src) = setup();
        let zip = zip_file(&src, "資料.zip", &[("テスト.txt".as_bytes(), b"x")], Enc::ZipCrypto("pw"));
        let result = state.classify_drop(std::slice::from_ref(&zip)).unwrap();
        let DropClassification::Extract { zip_path, zip_name, inspection } = result else {
            panic!("解凍モードになるはず");
        };
        assert_eq!(zip_path, zip.to_string_lossy());
        assert_eq!(zip_name, "資料.zip");
        assert!(inspection.encrypted);
        assert_eq!(inspection.detection, Detection::NotNeeded);
        // 判定しただけで、ワークフォルダには何も追加しない
        assert!(state.list_items().unwrap().is_empty());
    }

    #[test]
    fn 拡張子の大文字小文字は問わない() {
        let (state, _b, src) = setup();
        let zip = zip_file(&src, "A.ZIP", &[(b"a.txt", b"x")], Enc::None);
        assert!(matches!(state.classify_drop(&[zip]).unwrap(), DropClassification::Extract { .. }));
    }

    #[test]
    fn zip以外_複数_混在は圧縮モードになる() {
        let (state, _b, src) = setup();
        let txt = file(&src, "a.txt", "x");
        let zip = zip_file(&src, "b.zip", &[(b"a.txt", b"x")], Enc::None);
        let folder = src.path().join("dir");
        fs::create_dir(&folder).unwrap();

        for paths in [vec![txt.clone()], vec![folder], vec![zip.clone(), txt.clone()], vec![zip.clone(), zip.clone()]] {
            let result = state.classify_drop(&paths).unwrap();
            assert!(matches!(result, DropClassification::Compress { .. }), "{paths:?}");
        }
    }

    #[test]
    fn 一覧が空でなければzip1つでも圧縮モードのまま() {
        let (state, _b, src) = setup();
        state.add_items(&[file(&src, "a.txt", "x")], ConflictPolicy::Rename).unwrap();
        let zip = zip_file(&src, "b.zip", &[(b"a.txt", b"x")], Enc::None);
        assert!(matches!(state.classify_drop(&[zip]).unwrap(), DropClassification::Compress { .. }));
    }

    #[test]
    fn 壊れたzipはエラーになり_何も追加しない() {
        let (state, _b, src) = setup();
        let bad = file(&src, "bad.zip", "not a zip");
        assert!(matches!(state.classify_drop(&[bad]), Err(AppError::InvalidZip(_))));
        assert!(state.list_items().unwrap().is_empty());
    }

    #[test]
    fn 同名の衝突を返す() {
        let (state, _b, src) = setup();
        state.add_items(&[file(&src, "a.txt", "old")], ConflictPolicy::Rename).unwrap();

        // 既存と同名
        let again = file(&src, "a.txt", "new");
        let DropClassification::Compress { conflicts } = state.classify_drop(&[again]).unwrap() else { panic!() };
        assert_eq!(conflicts, ["a.txt"]);

        // ドロップの中で名前が重なる(別フォルダの同名ファイル)
        let x = file(&src, "x/same.txt", "1");
        let y = file(&src, "y/same.txt", "2");
        let DropClassification::Compress { conflicts } = state.classify_drop(&[x, y]).unwrap() else { panic!() };
        assert_eq!(conflicts, ["same.txt"]);

        // 衝突なし
        let other = file(&src, "other.txt", "3");
        let DropClassification::Compress { conflicts } = state.classify_drop(&[other]).unwrap() else { panic!() };
        assert!(conflicts.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn シンボリックリンクを含むドロップは全体を拒否する() {
        let (state, _b, src) = setup();
        let ok = file(&src, "ok.txt", "x");
        let dir = src.path().join("dir");
        fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", dir.join("link")).unwrap();

        assert!(matches!(state.classify_drop(&[ok.clone(), dir.clone()]), Err(AppError::SymlinkRejected(_))));
        assert!(matches!(state.add_items(&[ok, dir], ConflictPolicy::Rename), Err(AppError::SymlinkRejected(_))));
        assert!(state.list_items().unwrap().is_empty(), "一部だけ追加されない");
    }

    // ---- add / remove / clear ----

    #[test]
    fn 追加_削除_クリアができる() {
        let (state, _b, src) = setup();
        let a = file(&src, "a.txt", "1");
        let b = file(&src, "b.txt", "2");
        let items = state.add_items(&[a, b], ConflictPolicy::Rename).unwrap();
        assert_eq!(names(&items), ["a.txt", "b.txt"]);

        let items = state.remove_item("a.txt").unwrap();
        assert_eq!(names(&items), ["b.txt"]);
        assert!(matches!(state.remove_item("nothing"), Err(AppError::ItemNotFound(_))));

        state.clear().unwrap();
        assert!(state.list_items().unwrap().is_empty());
    }

    #[test]
    fn 同名はポリシーに従う() {
        let (state, _b, src) = setup();
        let a = file(&src, "a.txt", "old");
        state.add_items(std::slice::from_ref(&a), ConflictPolicy::Rename).unwrap();
        fs::write(&a, "new").unwrap();

        let items = state.add_items(std::slice::from_ref(&a), ConflictPolicy::Rename).unwrap();
        assert_eq!(names(&items), ["a.txt", "a_1.txt"]);

        let items = state.add_items(&[a], ConflictPolicy::Overwrite).unwrap();
        assert_eq!(names(&items), ["a.txt", "a_1.txt"]);
        let path = state.inner.workdir.path().join("a.txt");
        assert_eq!(fs::read_to_string(path).unwrap(), "new");
    }

    #[test]
    fn 途中で失敗したら_この呼び出しで追加した分を取り消す() {
        let (state, _b, src) = setup();
        let ok = file(&src, "ok.txt", "x");
        let missing = src.path().join("missing.txt");
        assert!(state.add_items(&[ok, missing], ConflictPolicy::Rename).is_err());
        assert!(state.list_items().unwrap().is_empty());
    }

    // ---- 処理中の排他とキャンセル ----

    #[test]
    fn 処理中は他の操作を受け付けない() {
        let (state, _b, src) = setup();
        let a = file(&src, "a.txt", "x");
        let (guard, _) = state.begin().unwrap();

        assert!(matches!(state.add_items(std::slice::from_ref(&a), ConflictPolicy::Rename), Err(AppError::Busy)));
        assert!(matches!(state.classify_drop(&[a]), Err(AppError::Busy)));
        assert!(matches!(state.remove_item("a.txt"), Err(AppError::Busy)));
        assert!(matches!(state.clear(), Err(AppError::Busy)));
        assert!(matches!(state.begin(), Err(AppError::Busy)));
        assert!(state.list_items().is_ok(), "一覧の取得は常にできる");

        drop(guard);
        assert!(state.clear().is_ok(), "処理が終われば受け付ける");
    }

    #[test]
    fn キャンセルは処理中のトークンにだけ効く() {
        let (state, _b, _src) = setup();
        state.cancel(); // 処理中でなければ、何も起きない
        let (guard, token) = state.begin().unwrap();
        assert!(!token.is_cancelled());
        state.cancel();
        assert!(token.is_cancelled());
        drop(guard);
        // 次の処理は、新しいトークンで始まる
        let (_g, next) = state.begin().unwrap();
        assert!(!next.is_cancelled());
    }

    // ---- 圧縮・解凍 ----

    #[test]
    fn 圧縮して_成功したらワークフォルダを空にする() {
        let (state, _b, src) = setup();
        let out = TempDir::new().unwrap();
        state
            .add_items(&[file(&src, "日本語.txt", "中身"), file(&src, "dir/b.txt", "b")], ConflictPolicy::Rename)
            .unwrap();

        let dest = out.path().join("out.zip");
        let mut last = None;
        let report = state
            .compress(&dest, TargetOs::Windows, None, &mut |p| last = Some(p))
            .unwrap();
        assert_eq!(report.file_count, 2);
        assert!(dest.is_file());
        assert!(state.list_items().unwrap().is_empty());
        assert!(last.is_some_and(|p| p.done == p.total));
    }

    #[test]
    fn 空の状態では圧縮できない() {
        let (state, _b, _src) = setup();
        let out = TempDir::new().unwrap();
        let result = state.compress(&out.path().join("o.zip"), TargetOs::Auto, None, &mut no_progress());
        assert!(matches!(result, Err(AppError::EmptyWorkDir)));
        assert!(!out.path().join("o.zip").exists());
    }

    #[test]
    fn 圧縮をキャンセルしたら_ワークフォルダは残る() {
        let (state, _b, src) = setup();
        let out = TempDir::new().unwrap();
        let big = "z".repeat(1024 * 1024);
        state
            .add_items(&[file(&src, "1.bin", &big), file(&src, "2.bin", &big)], ConflictPolicy::Rename)
            .unwrap();

        let dest = out.path().join("o.zip");
        let result = state.compress(&dest, TargetOs::Auto, None, &mut |p| {
            if p.done > 0 {
                state.cancel();
            }
        });
        assert!(matches!(result, Err(AppError::Cancelled)));
        assert!(!dest.exists());
        assert_eq!(state.list_items().unwrap().len(), 2, "やり直せるように残す");
        assert!(state.clear().is_ok(), "処理中のままにならない");
    }

    #[test]
    fn パスワード付きで圧縮して_そのまま解凍できる() {
        let (state, _b, src) = setup();
        let out = TempDir::new().unwrap();
        state.add_items(&[file(&src, "秘密.txt", "中身")], ConflictPolicy::Rename).unwrap();

        let dest = out.path().join("locked.zip");
        let enc = EncryptionArg { method: EncryptionMethod::Aes256, password: "pw".into() };
        state.compress(&dest, TargetOs::Auto, Some(enc), &mut no_progress()).unwrap();

        let none = state.extract(&dest, None, None, &mut no_progress());
        assert!(matches!(none, Err(AppError::PasswordRequired)));
        let wrong = state.extract(&dest, None, Some("x".into()), &mut no_progress());
        assert!(matches!(wrong, Err(AppError::WrongPassword)));
        let report = state.extract(&dest, None, Some("pw".into()), &mut no_progress()).unwrap();
        assert_eq!(fs::read_to_string(report.out_dir.join("秘密.txt")).unwrap(), "中身");
    }

    #[test]
    fn 解凍に文字コードを指定できる() {
        let (state, _b, src) = setup();
        // 「ア.txt」のUTF-8(フラグなし)は、文字コードが決められない
        let mut bytes = build_zip(&[("ア.txt".as_bytes(), b"1")], Enc::None);
        crate::test_util::clear_utf8_flag(&mut bytes);
        let zip = src.path().join("amb.zip");
        fs::write(&zip, bytes).unwrap();

        let DropClassification::Extract { inspection, .. } = state.classify_drop(std::slice::from_ref(&zip)).unwrap() else {
            panic!()
        };
        assert!(matches!(inspection.detection, Detection::Ambiguous { .. }));
        assert!(!inspection.preview.is_empty());

        let unresolved = state.extract(&zip, None, None, &mut no_progress());
        assert!(matches!(unresolved, Err(AppError::CharsetNotResolved)));
        let report = state.extract(&zip, Some(Charset::Utf8), None, &mut no_progress()).unwrap();
        assert!(report.out_dir.join("ア.txt").is_file());
    }

    // ---- 起動 ----

    #[test]
    fn 起動時に古いワークフォルダを掃除し_自分のは残す() {
        let base = TempDir::new().unwrap();
        let state = AppState::new(base.path()).unwrap();
        assert!(state.inner.workdir.path().is_dir());
        // 新しいもの(別の起動が使用中かもしれない)は消さない
        let other = AppState::new(base.path()).unwrap();
        assert!(state.inner.workdir.path().is_dir());
        assert!(other.inner.workdir.path().is_dir());
    }

    // ---- フロントエンドとの型 ----

    #[test]
    fn 判定結果と引数がjsonの形で受け渡せる() {
        let json = serde_json::to_value(DropClassification::Compress { conflicts: vec!["a".into()] }).unwrap();
        assert_eq!(json, serde_json::json!({ "kind": "compress", "conflicts": ["a"] }));

        let enc: EncryptionArg = serde_json::from_str(r#"{"method":"aes256","password":"pw"}"#).unwrap();
        assert_eq!(enc.method, EncryptionMethod::Aes256);
        let policy: ConflictPolicy = serde_json::from_str(r#""overwrite""#).unwrap();
        assert_eq!(policy, ConflictPolicy::Overwrite);
        let target: TargetOs = serde_json::from_str(r#""windows""#).unwrap();
        assert_eq!(target, TargetOs::Windows);
        let charset: Charset = serde_json::from_str(r#""shiftJis""#).unwrap();
        assert_eq!(charset, Charset::ShiftJis);
    }
}
