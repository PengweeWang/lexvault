#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod worker;

use ely_core::{
    Assets,
    theme::{Mode, Theme},
};
use gpui::{
    App, AppContext, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions,
    point, px, size,
};

actions!(lexvault_gui, [Quit, SelectNext, SelectPrevious]);

fn main() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            if let Err(error) = ely_core::init(cx) {
                eprintln!("无法初始化 Ely/GPUI：{error:#}");
                cx.quit();
                return;
            }

            Theme::set_mode_now(Mode::Dark, cx);
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("ctrl-q", Quit, None),
                KeyBinding::new("down", SelectNext, Some(app::KEY_CONTEXT)),
                KeyBinding::new("down", SelectNext, Some(app::SEARCH_CONTEXT)),
                KeyBinding::new("up", SelectPrevious, Some(app::KEY_CONTEXT)),
                KeyBinding::new("up", SelectPrevious, Some(app::SEARCH_CONTEXT)),
                KeyBinding::new("ctrl-n", SelectNext, Some(app::KEY_CONTEXT)),
                KeyBinding::new("ctrl-p", SelectPrevious, Some(app::KEY_CONTEXT)),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());

            let bounds = Bounds::centered(None, size(px(1120.0), px(760.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("LexVault".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.0), px(18.0))),
                }),
                window_min_size: Some(size(px(820.0), px(560.0))),
                app_owns_titlebar_drag: true,
                ..Default::default()
            };

            if let Err(error) = cx.open_window(options, |window, cx| {
                cx.new(|cx| app::LexVaultGui::new(window, cx))
            }) {
                eprintln!("无法打开 LexVault 窗口：{error:#}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
}
