fn main() {
    tauri_winres::WindowsResource::new()
        .set_icon_with_id("../../src-tauri/icons/icon.ico", "32512")
        .compile()
        .unwrap();
}
