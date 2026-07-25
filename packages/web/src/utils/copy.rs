use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

use crate::stores::ui_store::UiStore;

use super::copy_to_clipboard;

/// 创建复制按钮的 `onclick` handler（信号闪烁模式）。
///
/// - **成功**：`copied` 信号置 `true`，2 秒后自动复位
/// - **失败**（非 HTTPS 安全上下文）：通过 Toast 提示用户手动右键复制
///
/// # 用法
/// ```rust,ignore
/// onclick: on_copy(
///     text_to_copy.clone(),
///     i18n.t("xxx.copy_manual_hint").to_string(),
///     ui_store,
///     copied,
/// ),
/// ```
pub fn on_copy(
    text: String,
    hint: String,
    mut ui_store: UiStore,
    mut copied: Signal<bool>,
) -> EventHandler<MouseEvent> {
    EventHandler::new(move |_: MouseEvent| {
        if copy_to_clipboard(&text) {
            copied.set(true);
            let mut c = copied.clone();
            spawn(async move {
                TimeoutFuture::new(2000).await;
                c.set(false);
            });
        } else {
            ui_store.show_info(hint.as_str());
        }
    })
}

/// 创建复制按钮的 `onclick` handler（Toast 提示模式）。
///
/// 适用于按钮本身展示复制内容的场景（如邀请链接），复制成功时通过
/// Success Toast 反馈，而非改变按钮文本。
///
/// - **成功**：通过 Success Toast 提示用户
/// - **失败**（非 HTTPS 安全上下文）：通过 Info Toast 提示手动右键复制
///
/// # 用法
/// ```rust,ignore
/// onclick: on_copy_toast(
///     text_to_copy.clone(),
///     i18n.t("common.copied"),
///     i18n.t("common.copy_manual_hint"),
///     ui_store,
/// ),
/// ```
#[allow(dead_code)]
pub fn on_copy_toast(
    text: String,
    success_msg: &'static str,
    hint: &'static str,
    mut ui_store: UiStore,
) -> EventHandler<MouseEvent> {
    EventHandler::new(move |_: MouseEvent| {
        if copy_to_clipboard(&text) {
            ui_store.show_success(success_msg);
        } else {
            ui_store.show_info(hint);
        }
    })
}
