use crate::i18n::I18n;

/// 支付订单状态显示文本。未知状态保留原值，避免新增后端状态被错误隐藏。
pub fn payment_status_label(status: &str, i18n: &I18n) -> String {
    let key = match status {
        "paid" | "success" => Some("payment_orders.status_paid"),
        "pending" => Some("payment_orders.status_pending"),
        "processing" => Some("payment_orders.status_processing"),
        "failed" => Some("payment_orders.status_failed"),
        "closed" => Some("payment_orders.status_closed"),
        "cancelled" | "canceled" => Some("payment_orders.status_cancelled"),
        _ => None,
    };
    key.map(|key| i18n.t(key).to_string())
        .unwrap_or_else(|| status.to_string())
}

/// 支付渠道运行状态显示文本。
pub fn payment_provider_status_label(status: &str, i18n: &I18n) -> String {
    let key = match status {
        "available" => Some("payment_orders.provider_available"),
        "disabled" => Some("payment_orders.provider_disabled"),
        "misconfigured" => Some("payment_orders.provider_misconfigured"),
        "state_error" => Some("payment_orders.provider_state_error"),
        "configured_unverified" => Some("payment_orders.provider_unverified"),
        "unavailable" => Some("payment_orders.provider_unavailable"),
        "degraded" => Some("payment_orders.provider_degraded"),
        _ => None,
    };
    key.map(|key| i18n.t(key).to_string())
        .unwrap_or_else(|| status.to_string())
}

/// 支付渠道诊断说明由稳定状态码本地化，避免后端中文消息泄漏到英文界面。
pub fn payment_provider_message(status: &str, i18n: &I18n) -> Option<String> {
    let key = match status {
        "misconfigured" => "payment_orders.provider_misconfigured_message",
        "state_error" => "payment_orders.provider_state_error_message",
        "configured_unverified" => "payment_orders.provider_unverified_message",
        "unavailable" => "payment_orders.provider_unavailable_message",
        "degraded" => "payment_orders.provider_degraded_message",
        _ => return None,
    };
    Some(i18n.t(key).to_string())
}

pub fn usage_status_label(status: &str, i18n: &I18n) -> String {
    match status {
        "success" | "succeeded" => i18n.t("usage.status_success").to_string(),
        "failed" | "error" => i18n.t("usage.status_failed").to_string(),
        _ => status.to_string(),
    }
}

pub fn distribution_status_label(status: &str, i18n: &I18n) -> String {
    match status {
        "settled" | "paid" => i18n.t("distribution.status_settled").to_string(),
        "pending" => i18n.t("distribution.status_pending").to_string(),
        "cancelled" | "canceled" => i18n.t("distribution.status_cancelled").to_string(),
        "failed" => i18n.t("distribution.status_failed").to_string(),
        _ => status.to_string(),
    }
}

pub fn user_role_label(role: &str, i18n: &I18n) -> String {
    match role {
        "user" => i18n.t("users.role_user").to_string(),
        "admin" => i18n.t("users.role_admin").to_string(),
        "system" => i18n.t("users.role_system").to_string(),
        _ => role.to_string(),
    }
}

/// 表格中展示稳定的短 ID；完整值应由调用方放入 title 或复制操作。
pub fn short_id(value: &str) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(8).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::{
        payment_provider_message, payment_provider_status_label, payment_status_label, short_id,
        user_role_label,
    };
    use crate::i18n::{I18n, Lang};

    #[test]
    fn payment_statuses_are_localized_and_unknown_values_are_preserved() {
        let zh = I18n::new(Lang::Zh);
        let en = I18n::new(Lang::En);
        assert_eq!(payment_status_label("pending", &zh), "待支付");
        assert_eq!(payment_status_label("pending", &en), "Pending");
        assert_eq!(payment_status_label("future_state", &zh), "future_state");
        assert_eq!(payment_provider_status_label("disabled", &en), "Disabled");
        assert_eq!(
            payment_provider_message("configured_unverified", &en).as_deref(),
            Some("Configuration loaded; provider verification is still required before use.")
        );
        assert_eq!(payment_provider_message("available", &en), None);
        assert_eq!(user_role_label("system", &zh), "system（受保护）");
    }

    #[test]
    fn short_id_is_unicode_safe() {
        assert_eq!(short_id("1234567890"), "12345678…");
        assert_eq!(short_id("节点甲乙丙丁戊己庚辛壬"), "节点甲乙丙丁戊己…");
        assert_eq!(short_id("short"), "short");
    }
}
