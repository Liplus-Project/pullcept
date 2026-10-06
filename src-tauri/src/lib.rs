mod app_mcp;
mod character_editor;
mod codex_app_server;
mod codex_limit;
mod codex_status;
mod config;
mod pty;
mod room;
mod room_log;
mod session;
mod webhook;

use app_mcp::McpServers;
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
        // The MCP servers the app runs itself, by name (#172).
        .manage(McpServers::new())
        // The Codex seats stopped on their usage limit, and the posts the room
        // keeps back from them (#294).
        .manage(codex_limit::CodexLimits::new())
        // Each Codex seat's own app-server, by its terminal (#299).
        .manage(codex_app_server::CodexServers::new())
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
            // The servers listed in `mcp-servers.json`, the webhook bridge
            // among them: webhooks the app receives itself, posted into the
            // rooms (#169 / #172). Independent of the socket: a notice goes
            // through the room's own path, not over the wire.
            app_mcp::start_all(app.handle());
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
            config::account_env_text,
            config::seal_account_env,
            config::save_account_avatar,
            config::account_avatar,
            config::delete_account_avatar,
            config::save_sessions,
            config::load_sessions,
            room::room_port,
            room::room_participants,
            room::room_join,
            room::room_post,
            room::room_current_topic,
            room::room_new_topic,
            room::room_select_topic,
            room::room_delete_topic,
            room_log::room_topics,
            room_log::room_topic_log,
            room_log::room_rename_topic,
            room_log::room_forget_session,
            room_log::room_attach_path,
            room_log::room_attach_bytes,
            session::seated_accounts,
            session::parse_launch_options,
            session::launch_field_report,
            session::preview_launch_args,
            session::start_session,
            app_mcp::mcp_servers,
            app_mcp::save_mcp_server,
            app_mcp::create_mcp_server,
            app_mcp::delete_mcp_server,
            app_mcp::restart_mcp_server,
            app_mcp::open_mcp_servers_file,
            character_editor::open_character_file,
            character_editor::save_character_file,
            character_editor::character_file_wearers,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // However the app is left, no seat's app-server outlives it (#299).
            if let tauri::RunEvent::Exit = event {
                codex_app_server::stop_all(app);
            }
        });
}
