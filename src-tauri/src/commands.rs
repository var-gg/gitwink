#[tauri::command]
pub fn ping() -> &'static str {
    "pong"
}
