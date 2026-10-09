//! The links in a room post's words (#349): the commands a press on one comes
//! to. What may be opened and how is `crates/post-link`'s; this is only the
//! boundary that hands it to the opener.

use post_link::PathAction;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

/// Open a URL a post carries in the default browser, when it is `http://` or
/// `https://`. Checked here as well as on the screen: the screen's reading is
/// of the text, and a post is written by anyone in the room.
#[tauri::command]
pub fn open_post_url(app: AppHandle, url: String) -> Result<(), String> {
    let url = post_link::checked_url(&url)?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("{url} を開けませんでした: {e}"))
}

/// Open a path a post carries: a folder in Explorer, a file in the app its
/// type opens with, and a file that opening would run shown selected in its
/// folder instead. A path that is not there says 見つかりません.
///
/// Off the main thread: a UNC path asks a server whether it is there, and a
/// server that does not answer would hold the screen until it gives up.
#[tauri::command]
pub async fn open_post_path(app: AppHandle, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let opener = app.opener();
        let opened = match post_link::path_action(&path)? {
            PathAction::OpenFolder | PathAction::OpenFile => opener.open_path(&path, None::<&str>),
            PathAction::Reveal => opener.reveal_item_in_dir(&path),
        };
        opened.map_err(|e| format!("{path} を開けませんでした: {e}"))
    })
    .await
    .map_err(|e| format!("Failed to open the path: {e}"))?
}
