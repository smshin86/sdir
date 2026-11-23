use super::{
    AppModel, Bridge,
    lifecycle::{observe_placement, show_window},
};
use dioxus::{
    desktop::{DesktopContext, WindowCloseBehaviour},
    prelude::*,
};
use sdir::settings::SettingsWriter;
use std::sync::Arc;

pub(super) fn save_then_close(
    mut model: Signal<AppModel>,
    writer: &SettingsWriter,
    window: &DesktopContext,
    bridge: &Arc<Bridge>,
) {
    window.set_close_behavior(WindowCloseBehaviour::WindowHides);
    if model.read().closing {
        return;
    }
    if model.read().close_save_failed {
        bridge.search.cancel();
        window.set_close_behavior(WindowCloseBehaviour::WindowCloses);
        window.close();
        return;
    }
    let settings = {
        let mut state = model.write();
        if let Some(placement) = observe_placement(window, state.window.as_ref()) {
            state.window = Some(placement);
        }
        state.closing = true;
        state.save_due = None;
        state.settings()
    };
    let finished = writer.flush(settings);
    let window = window.clone();
    let bridge = bridge.clone();
    spawn(async move {
        let result = tokio::task::spawn_blocking(move || finished.recv()).await;
        if matches!(result, Ok(Ok(Ok(())))) {
            bridge.search.cancel();
            window.set_close_behavior(WindowCloseBehaviour::WindowCloses);
            window.close();
        } else {
            let mut state = model.write();
            state.closing = false;
            state.close_save_failed = true;
            state.notice =
                "마지막 설정을 저장하지 못했습니다. 다시 종료하면 저장하지 않고 닫습니다.".into();
            show_window(&window);
        }
    });
}
