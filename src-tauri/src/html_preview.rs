//! The trusted hosts for the document built by the main UI: one separate
//! window (#443) and one child view inside the main window's side pane (#466).
use std::sync::Mutex;
use tauri::{
    Manager, PhysicalPosition, PhysicalSize, Rect, Webview, WebviewBuilder, WebviewUrl,
    WebviewWindowBuilder,
};

const LABEL: &str = "html-preview";
/// The side pane's child view (#466). It sits in the `main` window, so the
/// main capability names the main webview (`webviews`), never the window: a
/// `windows: ["main"]` grant would reach this view too. No capability matches
/// this label, so an invoke from its frame is denied by the ACL.
pub const PANE_LABEL: &str = "html-preview-pane";
/// The fixed host page; `?pane` drops the separate window's header and note.
const HOST_PATH: &str = "/html-preview.html";
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
        // Not get_webview_window: once the pane's child view exists, `main`
        // holds two webviews and is no longer a webview window.
        if let Some(main) = app.get_window("main") {
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
    pane_document: Mutex<String>,
    pane_operations: tokio::sync::Mutex<()>,
}

/// Where the side pane's content area is, in physical pixels of the main
/// window's client area (the main webview fills it).
#[derive(serde::Deserialize)]
pub struct PaneBounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// One update of the side pane's child view: the document when it changed,
/// and its bounds, or none to hide it.
#[derive(serde::Deserialize)]
pub struct PanePlacement {
    document: Option<String>,
    bounds: Option<PaneBounds>,
}

fn draw_script(document: &str) -> String {
    // JSON encoding keeps source out of executable code, including quotes and
    // line breaks. The trusted host puts this string only in a sandbox iframe.
    let json = serde_json::to_string(document).expect("string serialization");
    format!("window.__htmlPreviewDocument={json};window.dispatchEvent(new Event('html-preview-update'))")
}

/// `pane` set = the call places the side pane's child view (#466), and
/// `document` / `open` are not read. Otherwise the call concerns the separate
/// window. One command for both keeps the app-command ACL of #449 as it was.
#[tauri::command]
pub async fn html_preview(
    webview: Webview,
    document: Option<String>,
    open: bool,
    pane: Option<PanePlacement>,
) -> Result<(), String> {
    // The webview's own label, not its window's: the pane's child view lives
    // in the `main` window and must never pass this check.
    if webview.label() != "main" {
        return Err("HTML preview is controlled by the main window".into());
    }
    if let Some(pane) = pane {
        return place_pane(&webview, pane).await;
    }
    let app = webview.app_handle();
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
        preview.eval(draw_script(&document)).map_err(|e| e.to_string())?;
        if open {
            preview.set_focus().map_err(|e| e.to_string())?;
        }
    } else {
        // Source changes update an existing detached view, but never reopen a
        // window the reader closed. Only its explicit open button creates one.
        if !open {
            return Ok(());
        }
        let origin = webview.url().map_err(|e| e.to_string())?.origin();
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
                    url.origin() == origin && url.path() == HOST_PATH
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
                        let _ = preview.eval(draw_script(&document));
                    }
                }
            })
            .build()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn place_pane(webview: &Webview, placement: PanePlacement) -> Result<(), String> {
    let app = webview.app_handle();
    let state = app.state::<HtmlPreview>();
    let _operation = state.pane_operations.lock().await;
    if let Some(document) = &placement.document {
        *state.pane_document.lock().map_err(|e| e.to_string())? = document.clone();
    }
    let bounds = placement
        .bounds
        .filter(|b| b.width > 0 && b.height > 0)
        .map(|b| Rect {
            position: PhysicalPosition::new(b.x, b.y).into(),
            size: PhysicalSize::new(b.width, b.height).into(),
        });
    let Some(pane) = app.get_webview(PANE_LABEL) else {
        // Nothing shown yet and nothing to show: no view to create.
        let Some(bounds) = bounds else { return Ok(()) };
        let origin = webview.url().map_err(|e| e.to_string())?.origin();
        let builder = WebviewBuilder::new(PANE_LABEL, WebviewUrl::App("html-preview.html?pane".into()))
            // Typing stays in the main view; the pane takes focus on a click.
            .focused(false)
            // The trusted top document never navigates to posted or external HTML.
            .on_navigation(move |url| url.origin() == origin && url.path() == HOST_PATH)
            .on_page_load(|pane, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    let document = pane
                        .state::<HtmlPreview>()
                        .pane_document
                        .lock()
                        .ok()
                        .map(|s| s.clone());
                    if let Some(document) = document {
                        let _ = pane.eval(draw_script(&document));
                    }
                }
            });
        webview
            .window()
            .add_child(builder, bounds.position, bounds.size)
            .map_err(|e| e.to_string())?;
        return Ok(());
    };
    if let Some(document) = &placement.document {
        pane.eval(draw_script(document)).map_err(|e| e.to_string())?;
    }
    match bounds {
        Some(bounds) => {
            pane.set_bounds(bounds).map_err(|e| e.to_string())?;
            pane.show().map_err(|e| e.to_string())?;
        }
        None => pane.hide().map_err(|e| e.to_string())?,
    }
    Ok(())
}

pub fn close(app: &tauri::AppHandle) {
    if let Some(preview) = app.get_webview_window(LABEL) {
        let _ = preview.destroy();
    }
}
