// An isolated native window harness: no room, config, relay, accounts or PTY.
// Uses the production command and shutdown helper with hidden test windows.
#[path = "../src/html_preview.rs"]
mod html_preview;
use tauri::Manager;

#[tauri::command]
// A window, not a WebviewWindow: with the side pane's child view (#466) the
// main window holds two webviews.
fn preview_test_close_main(window: tauri::Window) {
    assert_eq!(window.label(), "main");
    window.destroy().expect("destroy test main");
}

fn main() {
    let mut context = tauri::generate_context!();
    context.config_mut().identifier =
        format!("org.liplus-project.preview-check-{}", std::process::id());
    context.config_mut().build.dev_url = Some(
        std::env::var("PREVIEW_CHECK_URL")
            .expect("test URL")
            .parse()
            .expect("URL"),
    );
    let main = &mut context.config_mut().app.windows[0];
    main.url = tauri::WebviewUrl::App("html-preview.html".into());
    main.visible = false;
    tauri::Builder::default()
        .manage(html_preview::HtmlPreview::default())
        .invoke_handler(tauri::generate_handler![
            html_preview::html_preview,
            preview_test_close_main
        ])
        .on_page_load(|webview, _| {
            let _ = webview.window().hide();
        })
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                html_preview::close(window.app_handle());
            }
        })
        .run(context)
        .expect("native preview harness");
}
