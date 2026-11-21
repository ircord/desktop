use tauri::{
    AppHandle,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

use crate::{activate, updates};

pub fn register(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(
        app,
        "tray-open",
        "Open IRCord",
        true,
        None::<&str>,
    )?)?;
    if updates::available() {
        menu.append(&MenuItem::with_id(
            app,
            "tray-update",
            "Check for updates",
            true,
            None::<&str>,
        )?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "tray-quit",
        "Quit IRCord",
        true,
        None::<&str>,
    )?)?;

    let tray = TrayIconBuilder::with_id("ircord")
        .menu(&menu)
        .tooltip("IRCord")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray-open" => activate(app),
            "tray-update" => {
                activate(app);
                updates::manual(app);
            }
            "tray-quit" => app.exit(0),
            _ => {}
        });
    #[cfg(target_os = "macos")]
    let tray = tray
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/18x18.png"
        ))?)
        .icon_as_template(true);
    #[cfg(not(target_os = "macos"))]
    let tray = match app.default_window_icon() {
        Some(icon) => tray.icon(icon.clone()),
        None => tray,
    };
    tray.build(app)?;
    Ok(())
}
