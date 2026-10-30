use crate::review_types::ReviewRequest;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

#[derive(Default)]
pub struct PendingReview(Mutex<Option<ReviewRequest>>);

#[tauri::command]
pub fn take_pending_review(state: tauri::State<'_, PendingReview>) -> Option<ReviewRequest> {
    state.0.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[tauri::command]
pub async fn open_review(app: AppHandle, request: ReviewRequest) -> Result<(), String> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let result = (|| -> Result<(), String> {
            *handle
                .state::<PendingReview>()
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(request);
            if let Some(window) = handle.get_webview_window("review") {
                window.unminimize().ok();
                window.show().map_err(|e| e.to_string())?;
                window.set_focus().map_err(|e| e.to_string())?;
                handle
                    .emit_to("review", "review://pending", ())
                    .map_err(|e| e.to_string())?;
            } else {
                let window = WebviewWindowBuilder::new(
                    &handle,
                    "review",
                    WebviewUrl::App("index.html".into()),
                )
                .title("gitwink · Review")
                .inner_size(1100.0, 760.0)
                .min_inner_size(720.0, 480.0)
                .decorations(true)
                .resizable(true)
                .always_on_top(false)
                .skip_taskbar(false)
                .build()
                .map_err(|e| e.to_string())?;
                let close_handle = handle.clone();
                window.on_window_event(move |event| {
                    if matches!(event, WindowEvent::Destroyed) {
                        crate::window::assert_panel_always_on_top(&close_handle);
                    }
                });
            }
            crate::window::assert_panel_always_on_top(&handle);
            Ok(())
        })();
        let _ = tx.try_send(result);
    })
    .map_err(|e| e.to_string())?;
    rx.recv()
        .await
        .ok_or_else(|| "Review window dispatch failed".to_string())?
}
