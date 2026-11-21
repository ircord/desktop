fn main() {
    let manifest = tauri_build::AppManifest::new().commands(&["connect_server"]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("Build IRCord desktop metadata");
}
