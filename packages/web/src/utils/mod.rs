pub mod copy;
pub mod display;
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

/// CNY 支付金额的两位小数展示格式。
pub fn format_cny_str(value: &str) -> String {
    format!("¥{}", format_money_str(value))
}

/// 展示高精度金额：至少保留两位小数，同时保留所有有效小数位。
///
/// 节点小费、分销收益和模型计费金额使用 `DECIMAL(20,10)`，不能套用支付金额的
/// 两位截断规则。这里仅移除两位小数之后无意义的尾零，不改变后端返回的金额。
pub fn format_precise_money_str(value: &str) -> String {
    let Some((integer, fraction)) = value.split_once('.') else {
        return format!("{value}.00");
    };

    let mut fraction = fraction.trim_end_matches('0').to_string();
    while fraction.len() < 2 {
        fraction.push('0');
    }

    format!("{integer}.{fraction}")
}

/// CNY 高精度金额展示格式。
pub fn format_precise_cny_str(value: &str) -> String {
    format!("¥{}", format_precise_money_str(value))
}

#[cfg(test)]
mod tests {
    use super::{
        format_cny_str, format_money, format_money_str, format_precise_cny_str,
        format_precise_money_str,
    };

    #[test]
    fn money_display_uses_two_decimal_places_consistently() {
        assert_eq!(format_money_str("12"), "12.00");
        assert_eq!(format_money_str("12.3"), "12.30");
        assert_eq!(format_money_str("12.345"), "12.34");
        assert_eq!(format_money(9.999), "9.99");
        assert_eq!(format_cny_str("5.2"), "¥5.20");
    }

    #[test]
    fn precise_money_display_preserves_sub_cent_values_and_trims_only_trailing_zeros() {
        assert_eq!(format_precise_money_str("12"), "12.00");
        assert_eq!(format_precise_money_str("12.3"), "12.30");
        assert_eq!(format_precise_money_str("12.3400000000"), "12.34");
        assert_eq!(format_precise_money_str("0.0090000000"), "0.009");
        assert_eq!(format_precise_money_str("1.999"), "1.999");
        assert_eq!(format_precise_cny_str("0.0000000001"), "¥0.0000000001");
    }
}
