use dioxus::prelude::*;

/// 全局推荐码存储：用于在 /auth/register?ref=xxx 重定向到首页时传递推荐码
#[derive(Clone, Copy)]
pub struct ReferralStore {
    code: Signal<Option<String>>,
}

impl ReferralStore {
    pub fn new(code: Signal<Option<String>>) -> Self {
        Self { code }
    }

    /// 设置推荐码
    #[allow(dead_code)]
    pub fn set_code(&mut self, referral_code: String) {
        self.code.set(Some(referral_code));
    }

    /// 取出推荐码（取出后清空，避免重复使用）
    pub fn take_code(&mut self) -> Option<String> {
        let val = (self.code)().clone();
        if val.is_some() {
            self.code.set(None);
        }
        val
    }

    /// 查看推荐码（不清空）
    #[allow(dead_code)]
    pub fn peek_code(&self) -> Option<String> {
        (self.code)().clone()
    }

    /// 是否有待处理的推荐码
    #[allow(dead_code)]
    pub fn has_code(&self) -> bool {
        (self.code)().is_some()
    }
}
