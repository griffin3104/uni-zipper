mod commands;
mod encoding;
mod error;
mod exclude;
mod launch;
mod normalize;
mod progress;
#[cfg(test)]
mod test_util;
mod workdir;
mod zip_reader;
mod zip_writer;

use commands::AppState;
use tauri::Manager;
#[cfg(target_os = "macos")]
use tauri::Emitter;

/// 画面に、「開く要求が来た」ことを知らせる(中身は、`take_pending_open` で受け取る)。
/// macOSでしか使わない(Windows / Linuxは、起動時の引数で受け取るだけ)。
#[cfg(target_os = "macos")]
const OPEN_EVENT: &str = "open-paths";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // ワークフォルダを作り、前回の異常終了で残ったものを掃除する。
            let state = AppState::new(&workdir::default_base())?;
            // Windows / Linux: 「プログラムから開く」で、ファイルが起動時の引数に渡される。
            state.push_open_paths(launch::paths_from_args(std::env::args().skip(1)));
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::classify_drop,
            commands::add_items,
            commands::list_items,
            commands::remove_item,
            commands::clear_items,
            commands::compress,
            commands::extract,
            commands::cancel,
            commands::path_exists,
            commands::take_pending_open,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {
            // macOS: 「プログラムから開く」や、Zipをアプリのアイコンへドロップしたとき。
            // アプリが起動済みでも、起動前でも、このイベントで届く。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = _event {
                _app.state::<AppState>().push_open_paths(launch::paths_from_urls(&urls));
                let _ = _app.emit(OPEN_EVENT, ());
            }
        });
}
