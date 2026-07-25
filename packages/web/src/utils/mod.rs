pub mod copy;
pub mod time;

pub use copy::on_copy;

/// 复制文本到剪贴板（WASM 环境）
/// 返回 `true` 表示复制成功，`false` 表示不可用（非 HTTPS 上下文等）。
pub fn copy_to_clipboard(text: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let window = match web_sys::window() {
            Some(w) => w,
            None => return false,
        };
        let clipboard = window.navigator().clipboard();
        // navigator.clipboard 在非 HTTPS 下为 null/undefined
        let clipboard_ref: &wasm_bindgen::JsValue = clipboard.as_ref();
        if clipboard_ref.is_null() || clipboard_ref.is_undefined() {
            return false;
        }
        // write_text 返回 Promise，fire-and-forget 即可
        let _ = clipboard.write_text(text);
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = text;
        false
    }
}

/// 截断字符串金额到两位小数（直接截断，不四舍五入）
pub fn format_money_str(value: &str) -> String {
    if let Some(dot_pos) = value.find('.') {
        let end = (dot_pos + 3).min(value.len());
        let mut result = value[..end].to_string();
        // 补齐到两位小数
        if result.len() == dot_pos + 1 {
            result.push_str("00");
        } else if result.len() == dot_pos + 2 {
            result.push('0');
        }
        result
    } else {
        format!("{value}.00")
    }
}

/// 将 f64 截断到两位小数后格式化（直接截断，不四舍五入）
pub fn format_money(value: f64) -> String {
    format_money_str(&format!("{value:.10}"))
}
