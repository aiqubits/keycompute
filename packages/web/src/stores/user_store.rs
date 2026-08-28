use dioxus::prelude::*;

/// 当前用户信息
#[derive(Clone, PartialEq, Default)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub role: String,
    pub tenant_id: String,
}

impl UserInfo {
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.email)
    }

    #[allow(dead_code)]
    pub fn is_admin(&self) -> bool {
        self.role == "admin" || self.role == "system"
    }

    pub fn avatar_char(&self) -> char {
        self.display_name()
            .chars()
            .next()
            .map(|c| c.to_uppercase().next().unwrap_or(c))
            .unwrap_or('U')
    }
}

/// 用户信息 Store
#[derive(Clone, Copy)]
pub struct UserStore {
    pub info: Signal<Option<UserInfo>>,
    pub load_failed: Signal<bool>,
}

impl UserStore {
    /// 创建新的 UserStore。
    /// 注意：Signal 必须在组件顶层创建后传入
    pub fn new(info: Signal<Option<UserInfo>>, load_failed: Signal<bool>) -> Self {
        Self { info, load_failed }
    }

    #[allow(dead_code)]
    pub fn set(&mut self, user: UserInfo) {
        *self.info.write() = Some(user);
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        *self.info.write() = None;
        self.load_failed.set(false);
    }

    #[allow(dead_code)]
    pub fn get(&self) -> Option<UserInfo> {
        (self.info)()
    }

    #[allow(dead_code)]
    pub fn is_admin(&self) -> bool {
        (self.info)()
            .as_ref()
            .map(|u| u.is_admin())
            .unwrap_or(false)
    }
}
