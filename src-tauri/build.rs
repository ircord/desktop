fn main() {
    let manifest = tauri_build::AppManifest::new().commands(&[
        "connect_server",
        "notification_permission",
        "request_notification_permission",
        "show_notification",
        "focus_notification_window",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("Build IRCord desktop metadata");
}
