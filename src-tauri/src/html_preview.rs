//! A single trusted host window for the static document built by the main UI.
use std::sync::Mutex;
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const LABEL: &str = "html-preview";
/// Fixed navigation the host page makes for "メインウィンドウで開く" (#463).
/// The detached window holds no app-command capability, so this blocked
/// navigation is its only request path; the main window gets an event.
const RETURN_PATH: &str = "/html-preview-return";
pub const RETURN_EVENT: &str = "html-preview-return";

fn return_to_main(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<HtmlPreview>();
        let _operation = state.operations.lock().await;
        if let Some(preview) = app.get_webview_window(LABEL) {
            let _ = preview.destroy();
        }
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.unminimize();
            let _ = main.set_focus();
            let _ = tauri::Emitter::emit_to(&app, "main", RETURN_EVENT, ());
        }
    });
}

#[derive(Default)]
pub struct HtmlPreview {
    document: Mutex<String>,
    operations: tokio::sync::Mutex<()>,
}

fn draw(window: &WebviewWindow, document: &str) -> tauri::Result<()> {
    // JSON encoding keeps source out of executable code, including quotes and
    // line breaks. The trusted host puts this string only in a sandbox iframe.
    let json = serde_json::to_string(document).expect("string serialization");
    window.eval(&format!("window.__htmlPreviewDocument={json};window.dispatchEvent(new Event('html-preview-update'))"))
}

#[tauri::command]
pub async fn html_preview(
    window: WebviewWindow,
    document: Option<String>,
    open: bool,
) -> Result<(), String> {
    // Custom app commands are otherwise available to every app webview.
    // Existing capabilities stay unchanged; this command checks its own caller.
    if window.label() != "main" {
        return Err("HTML preview is controlled by the main window".into());
    }
    let app = window.app_handle();
    let state = app.state::<HtmlPreview>();
    // The native window survives JS controller recreation. Serialize across
    // controllers too, so a reload's clear follows an already in-flight open.
    let _operation = state.operations.lock().await;
    // Window creation is asynchronous command work, and no document lock spans a
    // native window call (page-load callbacks also read this state).
    *state.document.lock().map_err(|e| e.to_string())? = document.clone().unwrap_or_default();
    let Some(document) = document else {
        if let Some(preview) = app.get_webview_window(LABEL) {
            let (sender, destroyed) = tokio::sync::oneshot::channel();
            let sender = Mutex::new(Some(sender));
            preview.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    if let Some(sender) = sender.lock().ok().and_then(|mut s| s.take()) {
                        let _ = sender.send(());
                    }
                }
            });
            preview.destroy().map_err(|e| e.to_string())?;
            // destroy() only queues a native event. Acknowledging that event
            // prevents a queued reopen from finding the dying window's label.
            tokio::time::timeout(std::time::Duration::from_secs(5), destroyed)
                .await
                .map_err(|_| "HTML preview did not close in time".to_string())?
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    };
    if let Some(preview) = app.get_webview_window(LABEL) {
        draw(&preview, &document).map_err(|e| e.to_string())?;
        if open {
            preview.set_focus().map_err(|e| e.to_string())?;
        }
    } else {
        // Source changes update an existing detached view, but never reopen a
        // window the reader closed. Only its explicit open button creates one.
        if !open {
            return Ok(());
        }
        let origin = window.url().map_err(|e| e.to_string())?.origin();
        WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("html-preview.html".into()))
            .title("HTML プレビュー - Pullcept")
            .inner_size(900.0, 700.0)
            // This trusted top document never navigates to posted or external HTML.
            .on_navigation({
                let app = app.clone();
                move |url| {
                    if url.origin() == origin && url.path() == RETURN_PATH {
                        return_to_main(app.clone());
                        return false;
                    }
                    url.origin() == origin && url.path() == "/html-preview.html"
                }
            })
            .on_page_load(|preview, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    let document = preview
                        .state::<HtmlPreview>()
                        .document
                        .lock()
                        .ok()
                        .map(|s| s.clone());
                    if let Some(document) = document {
                        let _ = draw(&preview, &document);
                    }
                }
            })
            .build()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn close(app: &tauri::AppHandle) {
    if let Some(preview) = app.get_webview_window(LABEL) {
        let _ = preview.destroy();
    }
}
