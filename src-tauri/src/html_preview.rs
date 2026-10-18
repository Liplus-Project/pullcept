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
/// The separate window draws its own title bar (#468): `decorations(false)`,
/// as the main window. Its buttons and drag region cannot call the window API
/// (no capability reaches this window), so each is a fixed navigation under
/// this prefix that the guard blocks and carries out here, like RETURN_PATH.
const WINDOW_PATH: &str = "/html-preview-window/";

/// What the title bar asks for, by the segment after WINDOW_PATH.
#[derive(Clone, Copy, Debug, PartialEq)]
enum WindowAction {
    /// Pressed on the bar outside its buttons: the system's own move loop
    /// (Aero Snap included), as Tauri's `start_dragging`.
    Drag,
    /// A double click on the bar, and the maximize / restore button.
    ToggleMaximize,
    Minimize,
    Close,
}

impl WindowAction {
    fn from_path(path: &str) -> Option<Self> {
        match path.strip_prefix(WINDOW_PATH)? {
            "drag" => Some(Self::Drag),
            "toggle-maximize" => Some(Self::ToggleMaximize),
            "minimize" => Some(Self::Minimize),
            "close" => Some(Self::Close),
            _ => None,
        }
    }
}

fn window_action(app: tauri::AppHandle, action: WindowAction) {
    // Off the navigation callback, as return_to_main: the window is not
    // operated on from inside its own webview's event.
    tauri::async_runtime::spawn(async move {
        let Some(preview) = app.get_webview_window(LABEL) else {
            return;
        };
        let _ = match action {
            WindowAction::Drag => preview.start_dragging(),
            WindowAction::ToggleMaximize => match preview.is_maximized() {
                Ok(true) => preview.unmaximize(),
                _ => preview.maximize(),
            },
            WindowAction::Minimize => preview.minimize(),
            // The same close the standard frame's button asked for.
            WindowAction::Close => preview.close(),
        };
    });
}

/// Tells the host page whether its window is maximized, so the button names
/// what pressing it does now (最大化 / 元に戻す), however the size changed.
fn maximized_script(maximized: bool) -> String {
    format!("window.__htmlPreviewMaximized={maximized};window.dispatchEvent(new Event('html-preview-window'))")
}

fn send_maximized(preview: &tauri::WebviewWindow) {
    if let Ok(maximized) = preview.is_maximized() {
        let _ = preview.eval(maximized_script(maximized));
    }
}

/// The taskbar's and Alt+Tab's name for the window: the file's.
fn window_title(name: &str) -> String {
    if name.is_empty() {
        "Pullcept".into()
    } else {
        format!("{name} - Pullcept")
    }
}

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

/// What the separate window shows: the prepared document, and for its title
/// bar and code view the file name and the posted source (#468).
#[derive(Clone, Default)]
struct Shown {
    document: String,
    name: String,
    code: String,
}

#[derive(Default)]
pub struct HtmlPreview {
    document: Mutex<Shown>,
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

/// The separate window's update: the name and the source go first, as JSON
/// strings the host page writes only through textContent.
fn draw_window_script(shown: &Shown) -> String {
    let name = serde_json::to_string(&shown.name).expect("string serialization");
    let code = serde_json::to_string(&shown.code).expect("string serialization");
    format!(
        "window.__htmlPreviewName={name};window.__htmlPreviewCode={code};{}",
        draw_script(&shown.document)
    )
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
    name: Option<String>,
    code: Option<String>,
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
    let shown = Shown {
        document: document.clone().unwrap_or_default(),
        name: name.unwrap_or_default(),
        code: code.unwrap_or_default(),
    };
    *state.document.lock().map_err(|e| e.to_string())? = shown.clone();
    if document.is_none() {
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
    }
    if let Some(preview) = app.get_webview_window(LABEL) {
        preview.eval(draw_window_script(&shown)).map_err(|e| e.to_string())?;
        preview.set_title(&window_title(&shown.name)).map_err(|e| e.to_string())?;
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
        let preview = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("html-preview.html".into()))
            // The file name, for the taskbar and Alt+Tab; the window draws no
            // title of its own (#468).
            .title(window_title(&shown.name))
            // Its own title bar, as the main window (#252, #468).
            .decorations(false)
            .inner_size(900.0, 700.0)
            // This trusted top document never navigates to posted or external HTML.
            .on_navigation({
                let app = app.clone();
                move |url| {
                    if url.origin() != origin {
                        return false;
                    }
                    if url.path() == RETURN_PATH {
                        return_to_main(app.clone());
                        return false;
                    }
                    if let Some(action) = WindowAction::from_path(url.path()) {
                        window_action(app.clone(), action);
                        return false;
                    }
                    url.path() == HOST_PATH
                }
            })
            .on_page_load(|preview, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    let shown = preview
                        .state::<HtmlPreview>()
                        .document
                        .lock()
                        .ok()
                        .map(|s| s.clone());
                    if let Some(shown) = shown {
                        let _ = preview.eval(draw_window_script(&shown));
                    }
                    if let Some(window) = preview.app_handle().get_webview_window(LABEL) {
                        send_maximized(&window);
                    }
                }
            })
            .build()
            .map_err(|e| e.to_string())?;
        // Maximized by the button, a double click, Win+Up or a drag to the
        // screen's top edge alike: read back on every size change.
        let app = app.clone();
        preview.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Resized(_)) {
                if let Some(preview) = app.get_webview_window(LABEL) {
                    send_maximized(&preview);
                }
            }
        });
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
