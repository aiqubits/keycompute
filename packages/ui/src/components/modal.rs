use dioxus::prelude::*;

/// 对话框组件（受控模式）
///
/// # 示例
/// ```rust,ignore
/// let mut open = use_signal(|| false);
/// Modal {
///     open,
///     title: "确认删除",
///     onclose: move |_| *open.write() = false,
///     p { "确定要删除该项目吗？" }
/// }
/// ```
#[component]
pub fn Modal(
    /// 是否显示（受控）
    open: ReadSignal<bool>,
    /// 标题
    #[props(default)]
    title: String,
    /// 关闭回调（点击遮罩或关闭按钮触发）
    #[props(default)]
    onclose: EventHandler<()>,
    /// 关闭按钮的无障碍文案
    #[props(default = "Close".to_string())]
    close_label: String,
    /// 底部操作区（可选，若不传则不渲染 modal-footer）
    #[props(default)]
    footer: Option<Element>,
    /// 弹窗最大宽度（CSS 值，如 "640px"，默认 "480px"）
    #[props(default = "480px".to_string())]
    max_width: String,
    /// 主体内容
    children: Element,
) -> Element {
    if !open() {
        return rsx! {};
    }

    rsx! {
        div {
            class: "modal-backdrop",
            tabindex: "-1",
            onkeydown: move |event| {
                if event.key() == Key::Escape {
                    onclose.call(());
                }
            },
            onclick: move |_| onclose.call(()),
            div {
                class: "modal",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "app-modal-title",
                style: "max-width: {max_width}",
                // 阻止冒泡，避免点击内部关闭弹窗
                onclick: move |e| e.stop_propagation(),

                // 头部
                div { class: "modal-header",
                    h2 { id: "app-modal-title", class: "modal-title", "{title}" }
                    button {
                        class: "modal-close btn btn-ghost btn-sm",
                        r#type: "button",
                        autofocus: true,
                        aria_label: "{close_label}",
                        onclick: move |_| onclose.call(()),
                        "✕"
                    }
                }

                // 主体
                div { class: "modal-body",
                    {children}
                }

                // 底部（可选）
                if let Some(ftr) = footer {
                    div { class: "modal-footer",
                        {ftr}
                    }
                }
            }
        }
    }
}

/// 确认对话框（内置「确认」和「取消」按钮）
#[component]
pub fn ConfirmModal(
    /// 是否显示
    open: ReadSignal<bool>,
    /// 标题
    #[props(default = "确认操作".to_string())]
    title: String,
    /// 描述文字
    #[props(default)]
    message: String,
    /// 确认按钮文字
    #[props(default = "确认".to_string())]
    confirm_text: String,
    /// 取消按钮文字
    #[props(default = "取消".to_string())]
    cancel_text: String,
    /// 右上角关闭按钮的无障碍文案
    #[props(default = "Close".to_string())]
    close_label: String,
    /// 确认按钮是否为危险变体
    #[props(default = false)]
    danger: bool,
    /// 确认回调
    #[props(default)]
    onconfirm: EventHandler<()>,
    /// 取消/关闭回调
    #[props(default)]
    oncancel: EventHandler<()>,
) -> Element {
    if !open() {
        return rsx! {};
    }

    let confirm_class = if danger {
        "btn btn-danger"
    } else {
        "btn btn-primary"
    };

    rsx! {
        div {
            class: "modal-backdrop",
            tabindex: "-1",
            onkeydown: move |event| {
                if event.key() == Key::Escape {
                    oncancel.call(());
                }
            },
            onclick: move |_| oncancel.call(()),
            div {
                class: "modal",
                role: "alertdialog",
                aria_modal: "true",
                aria_labelledby: "confirm-modal-title",
                onclick: move |e| e.stop_propagation(),

                div { class: "modal-header",
                    h2 { id: "confirm-modal-title", class: "modal-title", "{title}" }
                    button {
                        class: "modal-close btn btn-ghost btn-sm",
                        r#type: "button",
                        autofocus: true,
                        aria_label: "{close_label}",
                        onclick: move |_| oncancel.call(()),
                        "✕"
                    }
                }

                div { class: "modal-body",
                    if !message.is_empty() {
                        p { class: "text-secondary", "{message}" }
                    }
                }

                div { class: "modal-footer",
                    button {
                        class: "btn btn-ghost",
                        r#type: "button",
                        onclick: move |_| oncancel.call(()),
                        "{cancel_text}"
                    }
                    button {
                        class: "{confirm_class}",
                        r#type: "button",
                        onclick: move |_| onconfirm.call(()),
                        "{confirm_text}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dialogs_expose_the_minimum_accessibility_contract() {
        let source = include_str!("modal.rs");
        assert!(source.contains("aria_modal: \"true\""));
        assert!(source.contains("aria_labelledby"));
        assert!(source.contains("aria_label:"));
        assert!(source.contains("aria_label: \"{close_label}\""));
        assert!(source.contains("Key::Escape"));
    }
}
