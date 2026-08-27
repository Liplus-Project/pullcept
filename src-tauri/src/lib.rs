mod config;
mod pty;
mod room;
mod room_log;
mod session;

use pty::PtyState;
use room::RoomState;
use session::RoomSeats;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(PtyState::new())
        .manage(RoomState::new())
        // Which account is in the room, so a second launch of one account is
        // refused rather than seating one identity twice (session::RoomSeats).
        .manage(RoomSeats::new())
        .setup(|app| {
            // The room has to be listening before any session is started: the
            // port goes into the `.mcp.json` a session launch writes.
            let handle = app.handle().clone();
            let state = handle.state::<RoomState>().inner().clone();
            tauri::async_runtime::spawn(async move {
                match room::start(handle, state).await {
                    Ok(port) => eprintln!("[room] listening on 127.0.0.1:{port}"),
                    Err(err) => eprintln!("[room] failed to start: {err}"),
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pty::spawn_pty,
            pty::write_pty,
            pty::resize_pty,
            pty::kill_pty,
            pty::kill_all_ptys,
            config::home_dir,
            config::load_config,
            config::save_config,
            config::save_sessions,
            config::load_sessions,
            room::room_port,
            room::room_participants,
            room::room_join,
            room::room_post,
            room::room_current_topic,
            room::room_new_topic,
            room::room_select_topic,
            room_log::room_topics,
            room_log::room_topic_log,
            room_log::room_rename_topic,
            session::seated_accounts,
            session::parse_launch_options,
            session::preview_launch_args,
            session::start_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
