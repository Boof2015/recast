use tauri::{
    menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu},
    AppHandle, Emitter, EventTarget, Manager, PhysicalPosition, PhysicalSize, WebviewWindow,
    WebviewWindowBuilder,
};
use tauri_plugin_dialog::DialogExt;

fn focused_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.webview_windows()
        .into_values()
        .find(|window| window.is_focused().unwrap_or(false))
}

fn cascade_position(
    anchor: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    work_origin: PhysicalPosition<i32>,
    work_size: PhysicalSize<u32>,
    scale: f64,
    occupied: &[PhysicalPosition<i32>],
) -> PhysicalPosition<i32> {
    let step = (28.0 * scale).round().max(1.0) as i64;
    let margin = (12.0 * scale).round() as i64;
    // Reduce the margin on tight displays; if the window cannot fit, keep its
    // top-left/title bar reachable rather than moving it beyond the work area.
    let spare_x = work_size.width.saturating_sub(size.width) as i64;
    let spare_y = work_size.height.saturating_sub(size.height) as i64;
    let left = work_origin.x as i64 + margin.min(spare_x / 2);
    let top = work_origin.y as i64 + margin.min(spare_y / 2);
    let right = work_origin.x as i64 + spare_x - margin.min(spare_x / 2);
    let bottom = work_origin.y as i64 + spare_y - margin.min(spare_y / 2);
    let mut x = (anchor.x as i64 + step).max(left);
    let mut y = (anchor.y as i64 + step).max(top);
    if x > right || y > bottom {
        x = left;
        y = top;
    }
    // A wrapped cascade may meet an older window. Try another visible slot
    // instead of placing its title bar exactly over that window's title bar.
    for _ in 0..=occupied.len() {
        if !occupied.iter().any(|position| {
            (position.x as i64 - x).abs() < step / 2 && (position.y as i64 - y).abs() < step / 2
        }) {
            break;
        }
        x += step;
        if x > right {
            x = left;
            y += step;
        }
        if y > bottom {
            y = top;
        }
    }
    PhysicalPosition::new(x as i32, y as i32)
}

pub fn new_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    // Use the same configuration as the first window, including platform
    // overrides, minimum size, and the development preview URL.
    let mut config = app.config().app.windows[0].clone();
    config.label = format!("batch-{}", uuid::Uuid::new_v4());
    let anchor = focused_window(app).and_then(|window| {
        Some((
            window.outer_position().ok()?,
            window.current_monitor().ok()??,
        ))
    });
    let occupied: Vec<_> = app
        .webview_windows()
        .into_values()
        .filter(|window| !window.is_minimized().unwrap_or(false))
        .filter_map(|window| window.outer_position().ok())
        .collect();
    let window = WebviewWindowBuilder::from_config(app, &config)?
        .visible(false)
        .focused(false)
        .build()?;
    if let Some((position, monitor)) = anchor {
        // Move while hidden, before measuring the actual outer frame on the
        // target display. Physical coordinates also support negative origins.
        if window.set_position(position).is_ok() {
            if let Ok(size) = window.outer_size() {
                let work = monitor.work_area();
                let position = cascade_position(
                    position,
                    size,
                    work.position,
                    work.size,
                    monitor.scale_factor(),
                    &occupied,
                );
                let _ = window.set_position(position);
            }
        }
    }
    window.show()?;
    window.set_focus()?;
    Ok(window)
}

pub fn install_menu(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::default(app)?;
    let file = menu.items()?.into_iter().find_map(|item| match item {
        MenuItemKind::Submenu(submenu) if submenu.text().is_ok_and(|text| text == "File") => {
            Some(submenu)
        }
        _ => None,
    });
    let file = match file {
        Some(file) => file,
        None => {
            let file = Submenu::with_items(
                app,
                "File",
                true,
                &[
                    &PredefinedMenuItem::close_window(app, None)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?;
            menu.insert(&file, 0)?;
            file
        }
    };
    file.insert_items(
        &[
            &MenuItem::with_id(app, "new-window", "New Window", true, Some("CmdOrCtrl+N"))?,
            &MenuItem::with_id(app, "add-files", "Add Files…", true, Some("CmdOrCtrl+O"))?,
            &PredefinedMenuItem::separator(app)?,
        ],
        0,
    )?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| match event.id().as_ref() {
        "new-window" => {
            if let Err(error) = new_window(app) {
                app.dialog()
                    .message(format!("A new window could not be opened. {error}"))
                    .title("Recast")
                    .show(|_| {});
            }
        }
        "add-files" => {
            if let Some(window) = focused_window(app) {
                // A targeted event prevents every open batch from opening a picker.
                let _ = app.emit_to(EventTarget::window(window.label()), "recast:add-files", ());
            }
        }
        _ => {}
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascade_keeps_a_logical_offset_on_scaled_and_negative_origin_displays() {
        for scale in [1.0, 1.5, 2.0] {
            let anchor = PhysicalPosition::new(-2700, 120);
            let result = cascade_position(
                anchor,
                PhysicalSize::new((1120.0 * scale) as u32, (620.0 * scale) as u32),
                PhysicalPosition::new(-3000, 48),
                PhysicalSize::new(3000, 1800),
                scale,
                &[anchor],
            );
            assert_eq!((result.x - anchor.x) as f64 / scale, 28.0);
            assert_eq!((result.y - anchor.y) as f64 / scale, 28.0);
        }
    }

    #[test]
    fn cascade_wraps_inside_the_work_area_and_skips_existing_windows() {
        let occupied = [PhysicalPosition::new(12, 60), PhysicalPosition::new(40, 60)];
        let result = cascade_position(
            PhysicalPosition::new(300, 230),
            PhysicalSize::new(1120, 620),
            PhysicalPosition::new(0, 48),
            PhysicalSize::new(1440, 832),
            1.0,
            &occupied,
        );
        assert!(!occupied.contains(&result));
        assert!(result.x >= 0 && result.x + 1120 <= 1440);
        assert!(result.y >= 48 && result.y + 620 <= 880);
    }

    #[test]
    fn cascade_keeps_the_title_bar_reachable_when_the_display_is_too_small() {
        let origin = PhysicalPosition::new(-800, 24);
        assert_eq!(
            cascade_position(
                PhysicalPosition::new(-700, 100),
                PhysicalSize::new(1120, 620),
                origin,
                PhysicalSize::new(800, 576),
                2.0,
                &[origin],
            ),
            origin,
        );
    }
}
