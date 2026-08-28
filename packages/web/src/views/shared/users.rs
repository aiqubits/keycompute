use client_api::{
    AdminApi, AssignableUserRole, UserRole,
    api::admin::{UpdateBalanceRequest, UpdateUserRequest, UserDetail, UserQueryParams},
};
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use ui::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, PageHeader, Pagination, Table,
    TableHead,
};

use crate::hooks::use_i18n::use_i18n;
use crate::router::Route;
use crate::services::api_client::{get_client, with_auto_refresh};
use crate::stores::auth_store::AuthStore;
use crate::stores::ui_store::UiStore;
use crate::stores::user_store::UserStore;
use crate::utils::display::{short_id, user_role_label};
use crate::utils::time::format_time;

const PAGE_SIZE: usize = 20;
const SEARCH_DEBOUNCE_MS: u32 = 300;

#[derive(Clone, Debug, Eq, PartialEq)]
struct UserListQuery {
    search: String,
    page: u32,
}

impl Default for UserListQuery {
    fn default() -> Self {
        Self {
            search: String::new(),
            page: 1,
        }
    }
}

impl UserListQuery {
    /// 提交新的搜索词时必须同时回到第一页，避免防抖窗口内的分页操作
    /// 被带入新查询。搜索词与页码放在同一状态中也只会触发一次资源重载。
    fn commit_search(&mut self, search: String) {
        self.search = search;
        self.page = 1;
    }
}

#[component]
pub fn Users() -> Element {
    let user_store = use_context::<UserStore>();
    let is_admin = user_store
        .info
        .read()
        .as_ref()
        .map(|u| u.is_admin())
        .unwrap_or(false);

    if is_admin {
        rsx! { AdminUsersView {} }
    } else {
        rsx! { UserSelfView {} }
    }
}

// ── Admin 视图 ────────────────────────────────────────────────────────

#[component]
fn AdminUsersView() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let auth_store = use_context::<AuthStore>();
    let mut ui_store = use_context::<UiStore>();
    let mut search = use_signal(String::new);
    let mut query = use_signal(UserListQuery::default);
    let current_user = user_store.info.read().clone();
    let can_current_user_manage_roles = current_user
        .as_ref()
        .map(|u| u.role == UserRole::System.as_str())
        .unwrap_or(false);
    let current_user_id = current_user
        .as_ref()
        .map(|u| u.id.clone())
        .unwrap_or_default();
    let current_user_id_for_edit = current_user_id.clone();
    let can_current_user_manage_roles_for_edit = can_current_user_manage_roles;
    let current_user_id_for_delete = current_user_id.clone();
    let can_current_user_delete_admins = can_current_user_manage_roles;

    // 编辑弹窗状态
    let mut edit_user = use_signal(|| Option::<UserDetail>::None);
    let mut edit_name = use_signal(String::new);
    let mut edit_role = use_signal(String::new);
    let mut edit_saving = use_signal(|| false);

    // 删除确认状态
    let mut delete_user = use_signal(|| Option::<UserDetail>::None);
    let mut delete_saving = use_signal(|| false);

    // 余额管理弹窗状态
    let mut balance_user = use_signal(|| Option::<UserDetail>::None);
    let mut balance_action = use_signal(|| "recharge".to_string());
    let mut balance_amount = use_signal(String::new);
    let mut balance_reason = use_signal(String::new);
    let mut balance_saving = use_signal(|| false);
    let mut balance_error = use_signal(String::new);

    use_effect(move || {
        let next_search = search();
        spawn(async move {
            TimeoutFuture::new(SEARCH_DEBOUNCE_MS).await;
            if search() == next_search && query.read().search != next_search {
                query.write().commit_search(next_search);
            }
        });
    });

    let mut users_resource = use_resource(move || {
        let current_query = query();
        async move {
            let params = UserQueryParams::new()
                .with_page_size(PAGE_SIZE as i64)
                .with_page(current_query.page as i64);
            let params = if !current_query.search.is_empty() {
                params.with_search(current_query.search)
            } else {
                params
            };
            with_auto_refresh(auth_store, move |token| {
                let params = params.clone();
                async move {
                    let client = get_client();
                    AdminApi::new(&client)
                        .list_all_users(Some(&params), &token)
                        .await
                }
            })
            .await
        }
    });

    let paged_users = move || -> Vec<UserDetail> {
        match users_resource() {
            Some(Ok(ref resp)) => resp.users.clone(),
            _ => vec![],
        }
    };

    let total_items = move || -> i64 {
        match users_resource() {
            Some(Ok(ref resp)) => resp.total,
            _ => 0,
        }
    };

    let total_pages = move || -> u32 {
        match users_resource() {
            Some(Ok(ref resp)) => resp.total_pages.max(1) as u32,
            _ => 1,
        }
    };

    // 提交编辑
    let on_edit_save = move |_| {
        let Some(u) = edit_user() else { return };
        let name_val = edit_name();
        let role_val = edit_role();
        let can_edit_role = can_current_user_manage_roles_for_edit
            && u.id != current_user_id_for_edit
            && u.role != "system";
        let role = if !can_edit_role || role_val.trim().is_empty() {
            None
        } else {
            match role_val.parse::<AssignableUserRole>() {
                Ok(role) => Some(role),
                Err(err) => {
                    ui_store.show_error(err);
                    return;
                }
            }
        };
        let id = u.id.clone();
        edit_saving.set(true);
        spawn(async move {
            let token = auth_store.token().unwrap_or_default();
            let client = get_client();
            let req = UpdateUserRequest {
                name: if name_val.trim().is_empty() {
                    None
                } else {
                    Some(name_val)
                },
                role,
            };
            match AdminApi::new(&client).update_user(&id, &req, &token).await {
                Ok(_) => {
                    ui_store.show_success(i18n.t("users.updated"));
                    edit_user.set(None);
                    users_resource.restart();
                }
                Err(e) => {
                    ui_store.show_error(format!("{}: {e}", i18n.t("users.update_failed")));
                }
            }
            edit_saving.set(false);
        });
    };

    // 确认删除
    let on_delete_confirm = move |_| {
        let Some(u) = delete_user() else { return };
        if u.id == current_user_id_for_delete
            || u.role == UserRole::System.as_str()
            || (u.role == UserRole::Admin.as_str() && !can_current_user_delete_admins)
        {
            // 区分不同类型的禁止删除原因
            let msg = if u.id == current_user_id_for_delete {
                i18n.t("users.delete_self_forbidden")
            } else if u.role == UserRole::System.as_str() {
                i18n.t("users.cannot_modify_system")
            } else {
                i18n.t("users.delete_admin_forbidden")
            };
            ui_store.show_error(msg.to_string());
            delete_user.set(None);
            return;
        }
        let id = u.id.clone();
        delete_saving.set(true);
        spawn(async move {
            let token = auth_store.token().unwrap_or_default();
            let client = get_client();
            match AdminApi::new(&client).delete_user(&id, &token).await {
                Ok(_) => {
                    ui_store.show_success(i18n.t("users.deleted"));
                    delete_user.set(None);
                    users_resource.restart();
                }
                Err(e) => {
                    ui_store.show_error(format!("{}: {e}", i18n.t("users.delete_failed")));
                }
            }
            delete_saving.set(false);
        });
    };

    // 提交余额操作
    let on_balance_save = move |_| {
        let Some(u) = balance_user() else { return };
        let action = balance_action();
        let amount_str = balance_amount();
        let reason = balance_reason();

        if amount_str.trim().is_empty() {
            balance_error.set(i18n.t("users.balance_amount_required").to_string());
            return;
        }
        let amount: f64 = match amount_str.trim().parse() {
            Ok(v) if v > 0.0 => v,
            _ => {
                balance_error.set(i18n.t("users.balance_amount_invalid").to_string());
                return;
            }
        };
        // 校验小数位不超过两位
        let trimmed = amount_str.trim();
        let valid_precision = match trimmed.find('.') {
            Some(pos) => trimmed.len() - pos - 1 <= 2,
            None => true,
        };
        if !valid_precision {
            balance_error.set(i18n.t("users.balance_amount_precision").to_string());
            return;
        }
        if reason.trim().is_empty() {
            balance_error.set(i18n.t("users.balance_reason_required").to_string());
            return;
        }

        let id = u.id.clone();
        balance_saving.set(true);
        spawn(async move {
            let token = auth_store.token().unwrap_or_default();
            let client = get_client();
            let result = match action.as_str() {
                "recharge" => {
                    let req = UpdateBalanceRequest::add(amount, &reason);
                    AdminApi::new(&client)
                        .update_user_balance(&id, &req, &token)
                        .await
                }
                "deduct" => {
                    let req = UpdateBalanceRequest::subtract(amount, &reason);
                    AdminApi::new(&client)
                        .update_user_balance(&id, &req, &token)
                        .await
                }
                "freeze" => {
                    let req = UpdateBalanceRequest::new(amount, &reason);
                    AdminApi::new(&client)
                        .freeze_user_balance(&id, &req, &token)
                        .await
                }
                "unfreeze" => {
                    let req = UpdateBalanceRequest::new(amount, &reason);
                    AdminApi::new(&client)
                        .unfreeze_user_balance(&id, &req, &token)
                        .await
                }
                _ => {
                    balance_error.set(i18n.t("users.balance_action_invalid").to_string());
                    balance_saving.set(false);
                    return;
                }
            };
            match result {
                Ok(_) => {
                    ui_store.show_success(i18n.t("users.balance_updated"));
                    balance_user.set(None);
                    users_resource.restart();
                }
                Err(e) => {
                    balance_error.set(format!("{}: {e}", i18n.t("users.balance_update_failed")));
                }
            }
            balance_saving.set(false);
        });
    };

    let edit_save_label = if edit_saving() {
        i18n.t("form.saving")
    } else {
        i18n.t("form.save")
    };
    let delete_button_label = if delete_saving() {
        i18n.t("users.deleting")
    } else {
        i18n.t("users.confirm_delete")
    };
    let can_edit_selected_role = edit_user()
        .as_ref()
        .map(|u| can_current_user_manage_roles && u.id != current_user_id && u.role != "system")
        .unwrap_or(false);
    let balance_save_label = move || -> String {
        if balance_saving() {
            i18n.t("form.saving").to_string()
        } else {
            i18n.t("form.confirm").to_string()
        }
    };
    let fmt_balance = |v: f64| crate::utils::format_money(v);

    rsx! {
        div { class: "page-container users-page",
        PageHeader {
            title: i18n.t("page.users").to_string(),
            description: i18n.t("users.subtitle").to_string(),
        }

        div { class: "toolbar",
            div { class: "toolbar-left",
                div { class: "input-wrapper search-input-wrapper",
                    input {
                        class: "input-field",
                        r#type: "search",
                        aria_label: i18n.t("users.search_placeholder"),
                        placeholder: "{i18n.t(\"users.search_placeholder\")}",
                        value: "{search}",
                        oninput: move |e| {
                            *search.write() = e.value();
                        },
                    }
                    if !search().is_empty() {
                        button {
                            class: "btn btn-ghost btn-sm input-clear-button",
                            r#type: "button",
                            aria_label: i18n.t("common.clear"),
                            onclick: move |_| {
                                search.set(String::new());
                                query.write().commit_search(String::new());
                            },
                            "×"
                        }
                    }
                }
            }
        }

        div { class: "card",
            {
                let (is_empty, empty_text) = match users_resource() {
                    None => (true, i18n.t("table.loading")),
                    Some(Err(_)) => (true, i18n.t("common.load_failed")),
                    Some(Ok(_)) if paged_users().is_empty() => (true, i18n.t("users.empty")),
                    _ => (false, ""),
                };
                rsx! {
                    Table {
                        empty: is_empty,
                        empty_text: empty_text.to_string(),
                        col_count: 6,
                        thead {
                            tr {
                                TableHead { {i18n.t("users.user")} }
                                TableHead { {i18n.t("table.role")} }
                                TableHead { {i18n.t("users.tenant")} }
                                TableHead { {i18n.t("users.balance")} }
                                TableHead { {i18n.t("users.registered_at")} }
                                TableHead { {i18n.t("table.actions")} }
                            }
                        }
                        tbody {
                            for u in paged_users().iter() {
                                tr {
                                    td {
                                        div { class: "user-cell",
                                            span { class: "user-name",
                                                { u.name.clone().unwrap_or_else(|| u.email.clone()) }
                                            }
                                            span { class: "user-email text-secondary", "{u.email}" }
                                        }
                                    }
                                    td {
                                        Badge { variant: BadgeVariant::Info, {user_role_label(&u.role, &i18n)} }
                                    }
                                    td { code { title: "{u.tenant_id}", {short_id(&u.tenant_id)} } }
                                    td {
                                        div { class: "balance-cell",
                                            span { class: "balance-available",
                                                "{fmt_balance(u.balance)}"
                                            }
                                            if u.frozen_balance > 0.0 {
                                                span { class: "balance-frozen text-secondary",
                                                    "({i18n.t(\"users.frozen_short\")} {fmt_balance(u.frozen_balance)})"
                                                }
                                            }
                                        }
                                    }
                                    td { { format_time(&u.created_at) } }
                                    td {
                                        div { class: "btn-group",
                                            // 仅 system 角色可编辑 system 用户；admin 可编辑其他用户
                                            if u.role != UserRole::System.as_str() || can_current_user_manage_roles {
                                                Button {
                                                    variant: ButtonVariant::Ghost,
                                                    size: ButtonSize::Small,
                                                    onclick: {
                                                        let uu = u.clone();
                                                        move |_| {
                                                            edit_name.set(uu.name.clone().unwrap_or_default());
                                                            edit_role.set(uu.role.clone());
                                                            edit_user.set(Some(uu.clone()));
                                                        }
                                                    },
                                                    {i18n.t("form.edit")}
                                                }
                                            }
                                            // 仅 system 角色可管理 system 用户的余额；admin 可管理其他用户
                                            if u.role != UserRole::System.as_str() || can_current_user_manage_roles {
                                                Button {
                                                    variant: ButtonVariant::Ghost,
                                                    size: ButtonSize::Small,
                                                    onclick: {
                                                        let uu = u.clone();
                                                        move |_| {
                                                            balance_action.set("recharge".to_string());
                                                            balance_amount.set(String::new());
                                                            balance_reason.set(String::new());
                                                            balance_error.set(String::new());
                                                            balance_user.set(Some(uu.clone()));
                                                        }
                                                    },
                                                    {i18n.t("users.balance_manage")}
                                                }
                                            }
                                            if u.id != current_user_id
                                                && u.role != UserRole::System.as_str()
                                                && (u.role != UserRole::Admin.as_str() || can_current_user_manage_roles) {
                                                Button {
                                                    variant: ButtonVariant::Danger,
                                                    size: ButtonSize::Small,
                                                    onclick: {
                                                        let uu = u.clone();
                                                        move |_| delete_user.set(Some(uu.clone()))
                                                    },
                                                    {i18n.t("form.delete")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        div { class: "pagination",
            span { class: "pagination-info",
                "{i18n.t(\"common.total_items\")} {total_items()} {i18n.t(\"pricing.items_suffix\")}"
            }
            Pagination {
                current: query().page,
                total_pages: total_pages(),
                previous_label: i18n.t("table.previous").to_string(),
                next_label: i18n.t("table.next").to_string(),
                on_page_change: move |p| query.write().page = p,
            }
        }

        // ── 编辑用户弹窗 ──────────────────────────────────────────
        if edit_user().is_some() {
            div { class: "modal-backdrop",
                onclick: move |_| edit_user.set(None),
                div {
                    class: "modal",
                    role: "dialog",
                    aria_modal: "true",
                    aria_label: i18n.t("users.edit_title"),
                    onclick: move |e| e.stop_propagation(),
                    div { class: "modal-header",
                        h2 { class: "modal-title", {i18n.t("users.edit_title")} }
                        button {
                            class: "btn btn-ghost btn-sm",
                            r#type: "button",
                            aria_label: i18n.t("common.close"),
                            onclick: move |_| edit_user.set(None),
                            "✕"
                        }
                    }
                    div { class: "modal-body",
                        div { class: "form-group",
                            label { class: "form-label", {i18n.t("users.display_name")} }
                            input {
                                class: "input-field",
                                placeholder: "{i18n.t(\"users.display_name_placeholder\")}",
                                value: "{edit_name}",
                                oninput: move |e| *edit_name.write() = e.value(),
                            }
                        }
                        div { class: "form-group",
                            label { class: "form-label", {i18n.t("table.role")} }
                            if can_edit_selected_role {
                                select {
                                    class: "input-field",
                                    value: "{edit_role}",
                                    onchange: move |e| *edit_role.write() = e.value(),
                                    option { value: "user", "{i18n.t(\"users.role_user\")}" }
                                    option { value: "admin", "{i18n.t(\"users.role_admin\")}" }
                                }
                            } else {
                                input {
                                    class: "input-field",
                                    value: "{edit_role}",
                                    readonly: true,
                                }
                            }
                        }
                    }
                    div { class: "modal-footer",
                        Button {
                            variant: ButtonVariant::Ghost,
                            onclick: move |_| edit_user.set(None),
                            {i18n.t("form.cancel")}
                        }
                        Button {
                            variant: ButtonVariant::Primary,
                            loading: edit_saving(),
                            onclick: on_edit_save,
                            "{edit_save_label}"
                        }
                    }
                }
            }
        }

        // ── 删除确认弹窗 ──────────────────────────────────────────
        if let Some(ref du) = delete_user() {
            div { class: "modal-backdrop",
                onclick: move |_| delete_user.set(None),
                div {
                    class: "modal",
                    role: "alertdialog",
                    aria_modal: "true",
                    aria_label: i18n.t("users.delete_confirm_title"),
                    onclick: move |e| e.stop_propagation(),
                    div { class: "modal-header",
                        h2 { class: "modal-title", {i18n.t("users.delete_confirm_title")} }
                    }
                    div { class: "modal-body",
                        p {
                            "{i18n.t(\"users.delete_confirm_prefix\")} "
                            strong { { du.name.clone().unwrap_or_else(|| du.email.clone()) } }
                            " ({du.email}) {i18n.t(\"users.delete_confirm_suffix\")}"
                        }
                    }
                    div { class: "modal-footer",
                        Button {
                            variant: ButtonVariant::Ghost,
                            onclick: move |_| delete_user.set(None),
                            {i18n.t("form.cancel")}
                        }
                        Button {
                            variant: ButtonVariant::Danger,
                            loading: delete_saving(),
                            onclick: on_delete_confirm,
                            "{delete_button_label}"
                        }
                    }
                }
            }
        }

        // ── 余额管理弹窗 ──────────────────────────────────────────
        if let Some(ref bu) = balance_user() {
            div { class: "modal-backdrop",
                onclick: move |_| {
                    balance_error.set(String::new());
                    balance_user.set(None);
                },
                div {
                    class: "modal",
                    role: "dialog",
                    aria_modal: "true",
                    aria_label: i18n.t("users.balance_title"),
                    onclick: move |e| e.stop_propagation(),
                    div { class: "modal-header",
                        h2 { class: "modal-title",
                            "{i18n.t(\"users.balance_title\")} - {bu.name.clone().unwrap_or_else(|| bu.email.clone())}"
                        }
                        button {
                            class: "btn btn-ghost btn-sm",
                            r#type: "button",
                            aria_label: i18n.t("common.close"),
                            onclick: move |_| {
                                balance_error.set(String::new());
                                balance_user.set(None);
                            },
                            "✕"
                        }
                    }
                    div { class: "modal-body",
                        // 弹窗内联错误提示
                        if !balance_error().is_empty() {
                            div { class: "modal-inline-error", "{balance_error}" }
                        }
                        // 当前余额展示
                        div { class: "balance-info",
                            div { class: "balance-row",
                                span { class: "balance-label", {i18n.t("users.balance_available")} }
                                span { class: "balance-value", "{fmt_balance(bu.balance)}" }
                            }
                            div { class: "balance-row",
                                span { class: "balance-label", {i18n.t("users.balance_frozen")} }
                                span { class: "balance-value", "{fmt_balance(bu.frozen_balance)}" }
                            }
                        }
                        // 操作选择
                        div { class: "form-group",
                            label { class: "form-label", {i18n.t("users.balance_action")} }
                            select {
                                class: "input-field",
                                value: "{balance_action}",
                                onchange: move |e| *balance_action.write() = e.value(),
                                option { value: "recharge", {i18n.t("users.balance_recharge")} }
                                option { value: "deduct", {i18n.t("users.balance_deduct")} }
                                option { value: "freeze", {i18n.t("users.balance_freeze")} }
                                option { value: "unfreeze", {i18n.t("users.balance_unfreeze")} }
                            }
                        }
                        // 金额输入
                        div { class: "form-group",
                            label { class: "form-label", {i18n.t("users.balance_amount")} }
                            input {
                                class: "input-field",
                                r#type: "number",
                                step: "0.01",
                                min: "0",
                                placeholder: "{i18n.t(\"users.balance_amount_placeholder\")}",
                                value: "{balance_amount}",
                                oninput: move |e| {
                                    *balance_amount.write() = e.value();
                                    balance_error.set(String::new());
                                },
                            }
                        }
                        // 原因输入
                        div { class: "form-group",
                            label { class: "form-label",
                                {i18n.t("users.balance_reason")}
                                span { class: "required-mark", " *" }
                            }
                            input {
                                class: "input-field",
                                placeholder: "{i18n.t(\"users.balance_reason_placeholder\")}",
                                value: "{balance_reason}",
                                oninput: move |e| {
                                    *balance_reason.write() = e.value();
                                    balance_error.set(String::new());
                                },
                            }
                        }
                    }
                    div { class: "modal-footer",
                        Button {
                            variant: ButtonVariant::Ghost,
                            onclick: move |_| balance_user.set(None),
                            {i18n.t("form.cancel")}
                        }
                        Button {
                            variant: ButtonVariant::Primary,
                            loading: balance_saving(),
                            onclick: on_balance_save,
                            "{balance_save_label()}"
                        }
                    }
                }
            }
        }
        }
    }
}

#[cfg(test)]
mod search_tests {
    use super::{SEARCH_DEBOUNCE_MS, UserListQuery};

    #[test]
    fn admin_search_uses_a_short_debounce() {
        assert!((250..=500).contains(&SEARCH_DEBOUNCE_MS));
    }

    #[test]
    fn committing_a_debounced_search_resets_any_intervening_page_change() {
        let mut query = UserListQuery {
            search: "old term".to_string(),
            page: 4,
        };

        // 模拟用户在防抖计时期间又点击了分页。
        query.page = 2;
        query.commit_search("new term".to_string());

        assert_eq!(
            query,
            UserListQuery {
                search: "new term".to_string(),
                page: 1,
            }
        );
    }
}

// ── 普通用户视图 ──────────────────────────────────────────────────────

#[component]
fn UserSelfView() -> Element {
    let i18n = use_i18n();
    let user_store = use_context::<UserStore>();
    let user_info = user_store.info.read();
    let nav = use_navigator();

    let display_name = user_info
        .as_ref()
        .map(|u| u.display_name().to_string())
        .unwrap_or_default();
    let email = user_info
        .as_ref()
        .map(|u| u.email.clone())
        .unwrap_or_default();
    let role = user_info
        .as_ref()
        .map(|u| u.role.clone())
        .unwrap_or_default();

    rsx! {
        div { class: "page-container",
        PageHeader {
            title: i18n.t("users.self_title").to_string(),
            description: i18n.t("users.self_desc").to_string(),
        }

        div { class: "card",
            div { class: "card-header",
                h3 { class: "card-title", {i18n.t("users.account_info")} }
                Button {
                    variant: ButtonVariant::Secondary,
                    size: ButtonSize::Small,
                    onclick: move |_| { nav.push(Route::UserProfile {}); },
                    {i18n.t("profile.edit")}
                }
            }
            div { class: "card-body",
                div { class: "info-grid",
                    div { class: "info-item",
                        span { class: "info-label", {i18n.t("users.display_name")} }
                        span { class: "info-value", "{display_name}" }
                    }
                    div { class: "info-item",
                        span { class: "info-label", {i18n.t("table.email")} }
                        span { class: "info-value", "{email}" }
                    }
                    div { class: "info-item",
                        span { class: "info-label", {i18n.t("table.role")} }
                        Badge { variant: BadgeVariant::Info, "{role}" }
                    }
                }
            }
        }
        }
    }
}
