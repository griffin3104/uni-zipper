mod encoding;
mod error;
mod exclude;
mod normalize;
mod progress;
#[cfg(test)]
mod test_util;
mod workdir;
mod zip_reader;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
