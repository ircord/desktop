use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, WebviewWindow};
use tauri_plugin_notification::NotificationExt;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    account_id: String,
    conversation_id: String,
}

pub fn listen(app: &AppHandle) -> tauri_plugin_notification::Result<()> {
    let handle = app.clone();
    app.notification().on_action(move |action| {
        if action.action_id() != "tap" {
            return;
        }
        let Some(value) = action
            .notification()
            .and_then(|notice| notice.extra().get("target"))
        else {
            return;
        };
        if let Ok(target) = serde_json::from_value::<Target>(value.clone()) {
            let _ = handle.emit_to("main", "ircord-notification-activated", target);
        }
    })
}

#[tauri::command]
pub async fn notification_permission(app: AppHandle) -> Result<&'static str, String> {
    #[cfg(target_os = "macos")]
    if macos::packaged() {
        return macos::permission().await;
    }
    permission_name(app.notification().permission_state())
}

#[tauri::command]
pub async fn request_notification_permission(app: AppHandle) -> Result<&'static str, String> {
    #[cfg(target_os = "macos")]
    if macos::packaged() {
        macos::request().await?;
        return macos::permission().await;
    }
    permission_name(app.notification().request_permission())
}

fn permission_name(
    permission: tauri_plugin_notification::Result<tauri_plugin_notification::PermissionState>,
) -> Result<&'static str, String> {
    use tauri_plugin_notification::PermissionState;
    Ok(
        match permission.map_err(|_| "Could not check notification permission.")? {
            PermissionState::Granted => "granted",
            PermissionState::Denied => "denied",
            _ => "default",
        },
    )
}

#[tauri::command]
pub fn show_notification(
    app: AppHandle,
    title: String,
    body: String,
    silent: bool,
    target: Option<Target>,
) -> Result<(), String> {
    let mut notification = app
        .notification()
        .builder()
        .title(title.chars().take(200).collect::<String>())
        .body(body.chars().take(200).collect::<String>())
        .extra("target", target);
    if !silent {
        notification = notification.sound("default");
    }
    notification
        .show()
        .map_err(|_| "Could not show the notification.".to_string())
}

#[tauri::command]
pub fn focus_notification_window(window: WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    window.unminimize()?;
    window.set_focus()
}

#[cfg(target_os = "macos")]
mod macos {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSError;
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSettings,
        UNUserNotificationCenter,
    };
    use std::{ptr::NonNull, sync::Mutex, time::Duration};
    use tokio::sync::oneshot;

    pub fn packaged() -> bool {
        // UserNotifications expects an application bundle.
        std::env::current_exe().is_ok_and(|path| {
            path.ancestors()
                .any(|part| part.extension().is_some_and(|ext| ext == "app"))
        })
    }

    pub async fn permission() -> Result<&'static str, String> {
        let response = {
            let (send, receive) = oneshot::channel();
            let send = Mutex::new(Some(send));
            let handler = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
                // The notification center keeps this value alive during the callback.
                let status = unsafe { settings.as_ref() }.authorizationStatus();
                let permission = match status {
                    UNAuthorizationStatus::NotDetermined => "default",
                    UNAuthorizationStatus::Denied => "denied",
                    _ => "granted",
                };
                if let Some(send) = send.lock().unwrap().take() {
                    let _ = send.send(permission);
                }
            });
            UNUserNotificationCenter::currentNotificationCenter()
                .getNotificationSettingsWithCompletionHandler(&handler);
            receive
        };
        tokio::time::timeout(Duration::from_secs(5), response)
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or_else(|| "Could not check notification permission.".into())
    }

    pub async fn request() -> Result<(), String> {
        let response = {
            let (send, receive) = oneshot::channel();
            let send = Mutex::new(Some(send));
            let handler = RcBlock::new(move |_: Bool, error: *mut NSError| {
                if let Some(send) = send.lock().unwrap().take() {
                    let _ = send.send(error.is_null());
                }
            });
            UNUserNotificationCenter::currentNotificationCenter()
                .requestAuthorizationWithOptions_completionHandler(
                    UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                    &handler,
                );
            receive
        };
        if response.await.unwrap_or(false) {
            Ok(())
        } else {
            Err("Could not request notification permission.".into())
        }
    }
}
