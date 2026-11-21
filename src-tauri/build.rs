fn main() {
    let manifest = tauri_build::AppManifest::new().commands(&[
        "check_server",
        "check_for_updates",
        "connect_server",
        "choose_server",
        "api_request",
        "cancel_request",
        "speech_request",
        "open_socket",
        "send_socket",
        "close_socket",
        "notification_permission",
        "request_notification_permission",
        "show_notification",
        "focus_notification_window",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("Build IRCord desktop metadata");
}
