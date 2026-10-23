use super::{dispatch_action, DesktopAction};
use tauri::{Manager, WebviewWindow};
use webview2_com::{
    AcceleratorKeyPressedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
        COREWEBVIEW2_PHYSICAL_KEY_STATUS,
    },
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_OEM_COMMA, VK_RWIN, VK_SHIFT,
};

fn shortcut(key: u32, control: bool, alt: bool, shift: bool, meta: bool) -> Option<DesktopAction> {
    if !control || alt || shift || meta {
        return None;
    }
    match key {
        0x4e => Some(DesktopAction::NewWindow),
        0x4f => Some(DesktopAction::AddFiles),
        0x4d => Some(DesktopAction::Minimize),
        key if key == u32::from(VK_OEM_COMMA) => Some(DesktopAction::Settings),
        _ => None,
    }
}

pub(super) fn install(window: &WebviewWindow) -> tauri::Result<()> {
    let app = window.app_handle().clone();
    let source = window.label().to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    window.with_webview(move |webview| {
        let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            // These interfaces are called on WebView2's owning UI thread.
            unsafe {
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                {
                    return Ok(());
                }
                let mut key = 0;
                args.VirtualKey(&mut key)?;
                let Some(action) = shortcut(
                    key,
                    GetKeyState(i32::from(VK_CONTROL)) < 0,
                    GetKeyState(i32::from(VK_MENU)) < 0,
                    GetKeyState(i32::from(VK_SHIFT)) < 0,
                    GetKeyState(i32::from(VK_LWIN)) < 0 || GetKeyState(i32::from(VK_RWIN)) < 0,
                ) else {
                    // Leave text editing, system shortcuts and other keys alone.
                    return Ok(());
                };
                // Consume before dispatch so the browser/native menu cannot
                // perform the action a second time. Holding a key is also consumed.
                args.SetHandled(true)?;
                let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
                args.PhysicalKeyStatus(&mut status)?;
                if !status.WasKeyDown.as_bool() {
                    dispatch_action(&app, Some(source.clone()), action);
                }
            }
            Ok(())
        }));
        let mut token = 0;
        // The controller owns the handler until this webview closes; the
        // handler captures only an app handle and label, not the controller.
        let result = unsafe {
            webview
                .controller()
                .add_AcceleratorKeyPressed(&handler, &mut token)
        };
        let _ = tx.send(result.map_err(|error| std::io::Error::other(error.to_string())));
    })?;
    rx.recv()
        .map_err(|_| tauri::Error::FailedToReceiveMessage)?
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_app_shortcuts_advertised_in_the_windows_menu() {
        for (key, action) in [
            (0x4e, DesktopAction::NewWindow),
            (0x4f, DesktopAction::AddFiles),
            (0x4d, DesktopAction::Minimize),
            (u32::from(VK_OEM_COMMA), DesktopAction::Settings),
        ] {
            assert_eq!(shortcut(key, true, false, false, false), Some(action));
            assert_eq!(shortcut(key, false, false, false, false), None);
            assert_eq!(shortcut(key, true, true, false, false), None);
            assert_eq!(shortcut(key, true, false, true, false), None);
            assert_eq!(shortcut(key, true, false, false, true), None);
        }
    }

    #[test]
    fn leaves_editing_and_unrelated_shortcuts_to_the_webview() {
        for key in [0x41, 0x43, 0x56, 0x58, 0x59, 0x5a, 0x25, 0x27, 0x73] {
            assert_eq!(shortcut(key, true, false, false, false), None);
        }
    }
}
