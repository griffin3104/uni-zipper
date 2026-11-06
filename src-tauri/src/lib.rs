mod commands;
mod encoding;
mod error;
mod exclude;
mod normalize;
mod progress;
#[cfg(test)]
mod test_util;
mod workdir;
mod zip_reader;
mod zip_writer;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            use tauri::Manager;
            // ワークフォルダを作り、前回の異常終了で残ったものを掃除する。
            app.manage(AppState::new(&workdir::default_base())?);
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
