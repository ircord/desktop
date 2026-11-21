use tauri::{AppHandle, WebviewWindow};

pub fn available() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
        && !tauri::is_dev()
        && option_env!("IRCORD_DESKTOP_UPDATES") == Some("1")
}

pub fn schedule(_app: &AppHandle) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if available() {
        let app = _app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            native::check(app, false).await;
        });
    }
}

pub fn manual(_app: &AppHandle) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if available() {
        tauri::async_runtime::spawn(native::check(_app.clone(), true));
    }
}

#[tauri::command]
pub async fn check_for_updates(_app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    if window.label() != "main" || !available() {
        return Err("Updates are unavailable in this build.".into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    native::check(_app, true).await;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native {
    use std::time::Duration;
    use tauri::{AppHandle, Manager};
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    use tauri_plugin_updater::UpdaterExt;
    use tokio::sync::{Mutex, oneshot};

    static CHECKING: Mutex<()> = Mutex::const_new(());

    pub async fn check(app: AppHandle, manual: bool) {
        let Ok(_checking) = CHECKING.try_lock() else {
            if manual {
                notice(
                    &app,
                    "IRCord is already checking for or installing an update.",
                    MessageDialogKind::Info,
                );
            }
            return;
        };
        let result = async {
            let builder = app.updater_builder().timeout(Duration::from_secs(30));
            #[cfg(target_os = "windows")]
            let builder = {
                use tauri_plugin_window_state::AppHandleExt;
                let app = app.clone();
                builder.on_before_exit(move || {
                    let _ = app.save_window_state(crate::WORKSPACE_STATE);
                    app.cleanup_before_exit();
                })
            };
            builder.build()?.check().await
        }
        .await;
        let update = match result {
            Ok(Some(update)) => update,
            Ok(None) => {
                if manual {
                    notice(
                        &app,
                        "You have the latest version of IRCord.",
                        MessageDialogKind::Info,
                    );
                }
                return;
            }
            Err(_) => {
                if manual {
                    notice(
                        &app,
                        "Could not check for an update. Try again later.",
                        MessageDialogKind::Error,
                    );
                }
                return;
            }
        };
        let (answer, response) = oneshot::channel();
        dialog(
            &app,
            format!(
                "IRCord {} is available. Download it and restart IRCord?",
                update.version
            ),
        )
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Download and restart".into(),
            "Later".into(),
        ))
        .show(move |confirmed| {
            let _ = answer.send(confirmed);
        });
        if response.await != Ok(true) {
            return;
        }

        let result = update.download_and_install(|_, _| {}, || {}).await;
        if result.is_err() {
            notice(
                &app,
                "Could not install the update. Try again or download the latest installer.",
                MessageDialogKind::Error,
            );
        } else {
            app.request_restart();
        }
    }

    fn dialog(
        app: &AppHandle,
        message: impl Into<String>,
    ) -> tauri_plugin_dialog::MessageDialogBuilder<tauri::Wry> {
        let mut dialog = app.dialog().message(message).title("IRCord update");
        if let Some(window) = app
            .get_webview_window("setup")
            .or_else(|| app.get_webview_window("main"))
        {
            dialog = dialog.parent(&window);
        }
        dialog
    }

    fn notice(app: &AppHandle, message: &str, kind: MessageDialogKind) {
        dialog(app, message).kind(kind).show(|_| {});
    }
}
