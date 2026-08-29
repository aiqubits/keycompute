use std::collections::HashMap;
use std::sync::LazyLock;

pub static ZH: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // ── 导航 ────────────────────────────────────
    m.insert("nav.home", "首页");
    m.insert("nav.usage", "用量统计");
    m.insert("nav.billing", "账单管理");
    m.insert("nav.api_keys", "API Keys");
    m.insert("nav.payments", "支付中心");
    m.insert("nav.payments.balance", "余额查询");
    m.insert("nav.payments.orders", "订单列表");
    m.insert("nav.payments.recharge", "充值");
    m.insert("nav.distribution", "分销中心");
    m.insert("nav.distribution.earnings", "分销收益");
    m.insert("nav.distribution.referrals", "推荐列表");
    m.insert("nav.distribution.invite", "邀请管理");
    m.insert("nav.user", "个人中心");
    m.insert("nav.user.profile", "个人资料");
    m.insert("nav.user.security", "安全设置");
    m.insert("nav.users", "用户管理");
    m.insert("nav.accounts", "账号管理");
    m.insert("nav.pricing", "定价管理");
    m.insert("nav.payment_orders", "支付订单");
    m.insert("nav.distribution_records", "分销记录");
    m.insert("nav.tenants", "租户管理");
    m.insert("nav.node_gateway", "节点网关");
    m.insert("nav.monitoring", "监控与诊断");
    m.insert("nav.account_settings", "账户设置");
    m.insert("nav.settings", "系统设置");
    m.insert("nav.group.usage", "用量");
    m.insert("nav.group.billing", "账务");
    m.insert("nav.group.account", "账户");
    m.insert("nav.group.admin", "管理");

    // ── 认证 ────────────────────────────────────
    m.insert("auth.login", "登录");
    m.insert("auth.register", "注册");
    m.insert("auth.logout", "退出登录");
    m.insert("auth.forgot_password", "忘记密码");
    m.insert("auth.reset_password", "重置密码");
    m.insert("auth.email", "邮箱");
    m.insert("auth.username", "用户名");
    m.insert("auth.password", "密码");
    m.insert("auth.confirm_password", "确认密码");
    m.insert("auth.name", "姓名");
    m.insert("auth.remember_me", "记住我");
    m.insert("auth.remember_me_hint", "关闭浏览器后仍保持登录");
    m.insert("auth.no_account", "还没有账号？");
    m.insert("auth.has_account", "已有账号？");
    m.insert("auth.send_reset_email", "发送重置邮件");
    m.insert("auth.back_to_login", "返回登录");
    m.insert("auth.login_subtitle", "登录您的账户以继续");
    m.insert("auth.register_subtitle", "创建您的账户");
    m.insert(
        "auth.reset_subtitle",
        "输入用户名和邮箱，校验通过后我们将发送重置链接",
    );
    m.insert(
        "auth.reset_sent",
        "如果用户名与邮箱匹配，重置链接已发送到对应邮箱，请注意查收",
    );
    m.insert("auth.register_now", "立即注册");
    m.insert("auth.login_now", "立即登录");
    m.insert("auth.email_placeholder", "admin@keycompute.local");
    // 占位密码 12345 与默认管理员密码保持一致，属产品有意为之，请勿在 review 中改动。
    // DO NOT CHANGE: intentional placeholder matching the default admin password.
    m.insert("auth.password_placeholder", "12345");
    m.insert("auth.username_placeholder", "请输入用户名");
    m.insert("auth.name_placeholder", "请输入姓名");
    m.insert("auth.confirm_password_placeholder", "再次输入密码");
    m.insert("auth.reset_email_placeholder", "请输入注册邮箱");
    m.insert("auth.reset_username_placeholder", "请输入注册用户名");
    m.insert("auth.fill_all", "请填写邮箱和密码");
    m.insert("auth.fill_required", "请填写所有必填项");
    m.insert("auth.enter_email", "请输入邮箱地址");
    m.insert("auth.enter_username", "请输入用户名");
    m.insert("auth.login_failed", "登录失败");
    m.insert("auth.register_failed", "注册失败");
    m.insert("auth.send_failed", "发送失败");
    m.insert("auth.sending", "发送中...");
    m.insert("auth.cooldown_retry", "请稍后重试");
    m.insert(
        "auth.identity_error",
        "邮箱地址或用户名错误，无法发送重置密码链接",
    );
    m.insert("auth.send_reset_link", "发送重置链接");
    m.insert("auth.logging_in", "登录中...");
    m.insert("auth.registering", "注册中...");
    m.insert("auth.request_code", "获取验证码");
    m.insert("auth.requesting_code", "验证码发送中...");
    m.insert("auth.request_code_failed", "获取验证码失败");
    m.insert("auth.resend_code", "重新发送验证码");
    m.insert("auth.complete_registration", "完成注册");
    m.insert("auth.verification_code", "邮箱验证码");
    m.insert("auth.verification_code_placeholder", "请输入 6 位验证码");
    m.insert("auth.code_required", "请输入邮箱验证码");
    m.insert("auth.code_sent_to", "验证码已发送至：");
    m.insert(
        "auth.code_sent_hint",
        "验证码 10 分钟内有效，验证成功后才会正式创建账号。",
    );
    m.insert("auth.change_email", "更换邮箱");
    m.insert(
        "auth.registration_success",
        "注册已完成，请使用刚刚设置的邮箱和密码登录。",
    );
    m.insert("auth.password_min8", "密码至少8位");

    // ── 页面标题 ─────────────────────────────────
    m.insert("page.home", "仪表盘");
    m.insert("page.usage", "用量统计");
    m.insert("page.billing", "账单管理");
    m.insert("page.api_keys", "API Key 管理");
    m.insert("page.payments", "支付中心");
    m.insert("page.distribution", "分销中心");
    m.insert("page.profile", "个人资料");
    m.insert("page.security", "安全设置");
    m.insert("page.users", "用户管理");
    m.insert("page.accounts", "账号管理");
    m.insert("page.pricing", "定价管理");
    m.insert("page.payment_orders", "支付订单");
    m.insert("page.distribution_records", "分销记录");
    m.insert("page.tenants", "租户管理");
    m.insert("page.account_settings", "账户设置");
    m.insert("page.settings", "系统设置");
    m.insert("page.node_gateway", "节点网关");
    m.insert("page.monitoring", "监控与诊断");
    m.insert("page.not_found", "页面不存在");

    // ── 表单 ────────────────────────────────────
    m.insert("form.save", "保存");
    m.insert("form.cancel", "取消");
    m.insert("form.confirm", "确认");
    m.insert("form.delete", "删除");
    m.insert("form.create", "新建");
    m.insert("form.edit", "编辑");
    m.insert("form.search", "搜索");
    m.insert("form.reset", "重置");
    m.insert("form.submit", "提交");
    m.insert("form.saving", "保存中...");
    m.insert("form.save_changes", "保存修改");
    m.insert("form.required", "此字段为必填项");
    m.insert("form.invalid_email", "请输入有效的邮箱地址");
    m.insert("form.password_too_short", "密码至少 8 位");
    m.insert("form.password_mismatch", "两次密码不一致");

    // ── 表格 ────────────────────────────────────
    m.insert("table.no_data", "暂无数据");
    m.insert("table.loading", "加载中...");
    m.insert("table.previous", "‹ 上一页");
    m.insert("table.next", "下一页 ›");
    m.insert("table.actions", "操作");
    m.insert("table.status", "状态");
    m.insert("table.created_at", "创建时间");
    m.insert("table.name", "名称");
    m.insert("table.email", "邮箱");
    m.insert("table.role", "角色");

    // ── 通用 ────────────────────────────────────
    m.insert("common.loading", "加载中");
    m.insert("common.more", "更多");
    m.insert("common.error", "出错了");
    m.insert("common.success", "操作成功");
    m.insert("common.confirm_delete", "确定要删除吗？此操作不可撤销。");
    m.insert("common.copied", "已复制到剪贴板");
    m.insert("common.copy", "复制");
    m.insert(
        "common.copy_manual_hint",
        "当前页面为非安全上下文（HTTP），请选中文本后右键复制",
    );
    m.insert("common.refresh", "刷新");
    m.insert("common.back", "返回");
    m.insert("common.clear", "清空");
    m.insert("common.close", "关闭");
    m.insert("common.time", "时间");
    m.insert("common.total_items", "共");
    m.insert("common.range", "范围");
    m.insert("common.created_at_label", "创建于");
    m.insert("common.load_failed", "加载失败");
    m.insert(
        "common.user_info_load_failed",
        "用户信息加载失败，无法验证管理员权限。",
    );
    m.insert("common.retry", "重试");
    m.insert("common.redirecting", "跳转中");
    m.insert("common.redirect_to_login", "正在跳转到登录页…");
    m.insert("common.redirect_to_home", "正在跳转到首页…");
    m.insert("common.admin_only_page", "权限不足：该页面仅管理员可访问");
    m.insert("common.expand", "展开");
    m.insert("common.collapse", "折叠");
    m.insert("common.enabled", "已启用");
    m.insert("common.disabled", "已禁用");
    m.insert("common.yes", "是");
    m.insert("common.no", "否");
    m.insert("common.admin", "管理员");
    m.insert("common.user", "普通用户");
    m.insert("common.no_permission", "您没有权限访问此页面");
    m.insert("common.balance", "余额");
    m.insert("common.amount", "金额");
    m.insert("common.currency", "货币");
    m.insert("common.tokens", "Token 数");
    m.insert("common.requests", "请求数");
    m.insert("common.cost", "费用");
    m.insert("dashboard.greeting", "你好");
    m.insert("dashboard.subtitle", "这是您的控制台概览");
    m.insert("dashboard.api_calls", "API 调用次数");
    m.insert("dashboard.weekly_total", "本周累计");
    m.insert("dashboard.balance", "账户余额");
    m.insert("dashboard.available", "可用");
    m.insert("dashboard.active_keys", "活跃 Key");
    m.insert("dashboard.total", "总计");
    m.insert("dashboard.weekly_cost", "本周消耗");
    m.insert("dashboard.used", "已用");
    m.insert("dashboard.quick_links", "快速入口");
    m.insert("dashboard.manage_api_keys", "管理 API Key");
    m.insert("dashboard.recharge", "充値余额");
    m.insert("dashboard.account_settings", "账户设置");
    m.insert(
        "api_keys.subtitle",
        "管理 OpenAI 兼容接口访问密钥。密钥明文仅在创建成功后显示一次。",
    );
    m.insert("api_keys.create", "创建 API Key");
    m.insert("api_keys.active", "活跃");
    m.insert("api_keys.all_with_revoked", "全部（含已撤销）");
    m.insert("api_keys.created_title", "API Key 已创建");
    m.insert("api_keys.created_once", "仅显示一次，请立即保存。");
    m.insert("api_keys.example", "使用示例");
    m.insert("api_keys.models_title", "1. 当前系统可用模型");
    m.insert("api_keys.models_desc_prefix", "可在请求中通过 ");
    m.insert("api_keys.models_desc_suffix", " 参数使用");
    m.insert("api_keys.default_model", "默认");
    m.insert("api_keys.more_models", "个更多模型");
    m.insert(
        "api_keys.quick_example",
        "2. 快速使用示例（可直接复制使用）",
    );
    m.insert(
        "api_keys.quick_example_desc",
        "复制以下示例并替换成你的业务代码即可快速接入。",
    );
    m.insert("api_keys.example_env", "环境变量");
    m.insert(
        "api_keys.example_env_comment",
        "将以下内容添加到你的 .env 文件或环境变量中",
    );
    m.insert("api_keys.example_python", "OpenAI SDK（Python）");
    m.insert("api_keys.example_node", "OpenAI SDK（Node.js）");
    m.insert("api_keys.example_curl", "cURL");
    m.insert("api_keys.example_protocol_openai", "OpenAI");
    m.insert("api_keys.example_protocol_anthropic", "Anthropic");
    m.insert(
        "api_keys.example_python_anthropic",
        "Anthropic SDK（Python）",
    );
    m.insert(
        "api_keys.example_node_anthropic",
        "Anthropic SDK（Node.js）",
    );
    m.insert(
        "api_keys.example_note_anthropic",
        "提示：Anthropic 示例调用 /v1/messages 接口；示例模型优先取列表中第一个 Claude 模型，无 Claude 时取列表第一个模型；若列表为空，示例中的 {model} 为不可用占位，请替换为实际可用的模型。",
    );
    m.insert("api_keys.copy", "复制");
    m.insert("api_keys.copy_hint", "点击复制");
    m.insert("api_keys.copied", "已复制");
    m.insert(
        "api_keys.example_note",
        "将以上配置用于 OpenAI 兼容的 SDK 或工具中。",
    );
    m.insert(
        "api_keys.example_note_prefix",
        "提示：如需更换模型，只需修改 ",
    );
    m.insert(
        "api_keys.example_note_suffix",
        " 的值为左侧列表中的任一模型即可。",
    );
    m.insert("api_keys.close_saved", "我已记录，关闭");
    m.insert("api_keys.create_title", "创建 API Key");
    m.insert("api_keys.name", "名称");
    m.insert("api_keys.name_placeholder", "为此 Key 取个名字");
    m.insert("api_keys.creating", "创建中...");
    m.insert("api_keys.create_failed", "创建失败");
    m.insert("api_keys.delete_confirm_title", "删除 API Key");
    m.insert(
        "api_keys.delete_confirm_message",
        "确定删除 API Key“{name}”吗？删除后无法恢复。",
    );
    m.insert("api_keys.loading_failed", "加载失败");
    m.insert("api_keys.registry", "API 密钥管理");
    m.insert("api_keys.empty_meta", "当前筛选条件下没有可用凭证。");
    m.insert("api_keys.active_meta", "正在显示可用于调用网关的活跃凭证。");
    m.insert("api_keys.all_meta", "正在显示全部凭证，包括已撤销记录。");
    m.insert("api_keys.empty", "暂无可用的 API Key，点击上方按钮创建");
    m.insert("api_keys.prefix", "前缀");
    m.insert("api_keys.revoked", "已撤销");

    // ── Layout ──────────────────────────────────
    m.insert("layout.back_to_home", "返回首页");
    m.insert("layout.open_menu", "打开菜单");
    m.insert("layout.close_menu", "关闭菜单");
    m.insert("layout.user_menu", "用户菜单");
    m.insert("layout.switch_to_light", "切换到亮色主题");
    m.insert("layout.switch_to_dark", "切换到暗色主题");
    m.insert("layout.switch_to_zh", "切换到中文");
    m.insert("layout.switch_to_en", "切换到英文");
    m.insert("layout.expand_sidebar", "展开侧边栏");
    m.insert("layout.collapse_sidebar", "折叠侧边栏");

    // ── Error ───────────────────────────────────
    m.insert("error.not_found_desc", "您访问的页面不存在或已被移除");
    m.insert("error.back_home", "返回首页");

    // ── Home ────────────────────────────────────
    m.insert("home.welcome", "欢迎使用 KeyCompute");
    m.insert("home.login", "登录");
    m.insert("home.register", "注册");
    m.insert("home.console", "控制台");
    m.insert("home.toggle_theme", "切换主题");
    m.insert("home.features.title", "核心优势");
    m.insert("home.features.routing.title", "智能路由");
    m.insert(
        "home.features.routing.desc",
        "多模型智能调度，自动选择最优路径",
    );
    m.insert("home.features.billing.title", "实时计费");
    m.insert("home.features.billing.desc", "精准计量，秒级结算，透明消费");
    m.insert("home.features.cluster.title", "分布式集群");
    m.insert(
        "home.features.cluster.desc",
        "多区域部署，弹性扩展，高可用保障",
    );
    m.insert("home.features.node_rental.title", "节点租赁");
    m.insert(
        "home.features.node_rental.desc",
        "个人 PC 接入算力市场，闲置资源变现",
    );
    m.insert("home.features.distribution.title", "传播裂变");
    m.insert(
        "home.features.distribution.desc",
        "二级分销佣金，推荐奖励，增长飞轮",
    );
    m.insert("home.features.custom.title", "需求定制");
    m.insert(
        "home.features.custom.desc",
        "按需定制，灵活适配企业方业务场景",
    );
    m.insert("home.menu", "菜单");
    m.insert("home.lang_switch_label", "EN");
    m.insert("home.tip.title", "赞赏支持");
    m.insert("home.tip.subtitle_prefix", "如果您喜欢 ");
    m.insert("home.tip.subtitle_suffix", "，欢迎请我们喝杯咖啡 ☕");
    m.insert("home.tip.wechat_qr_alt", "微信赞赏码");
    m.insert("home.tip.wechat_label", "微信赞赏");
    m.insert("home.tip.alipay_qr_alt", "支付宝赞赏码");
    m.insert("home.tip.alipay_label", "支付宝赞赏");
    m.insert("home.contributors.title", "社区贡献者");
    m.insert(
        "home.contributors.subtitle",
        "感谢所有为 KeyCompute 做出贡献的开发者 ❤️",
    );
    m.insert("home.sponsors.title", "社区赞助者");
    m.insert(
        "home.sponsors.subtitle",
        "感谢以下朋友的赞赏支持 🙏（排名不分先后）",
    );

    // ── 需求收集 ─────────────────────────────────
    m.insert("req.bubble", "需求咨询");
    m.insert("req.title", "提交算力需求");
    m.insert("req.subtitle", "告诉我们你的需求，专业团队将尽快与您联系");
    m.insert("req.single_choice", "单选");
    m.insert("req.type.label", "需求类型");
    m.insert("req.type.api", "API 调用");
    m.insert("req.type.private", "私有部署");
    m.insert("req.type.rental", "节点租赁");
    m.insert("req.type.distributed", "分布式推理");
    m.insert("req.type.cost", "成本优化");
    m.insert("req.type.other", "其他");
    m.insert("req.model.label", "模型需求");
    m.insert("req.model.placeholder", "请选择或输入模型名称");
    m.insert("req.scale.label", "预计使用规模");
    m.insert("req.scale.test", "测试阶段");
    m.insert("req.scale.lt1w", "每天 1 万 tokens 以下");
    m.insert("req.scale.10w", "每天 10 万 tokens");
    m.insert("req.scale.100w", "每天 100 万 tokens+");
    m.insert("req.scale.unknown", "不确定");
    m.insert("req.deploy.label", "节点部署方案");
    m.insert("req.deploy.image", "容器镜像部署");
    m.insert("req.deploy.recommended", "推荐");
    m.insert(
        "req.deploy.image_desc",
        "预置镜像，一键拉起运行；快速部署，便于标准运维",
    );
    m.insert("req.deploy.binary", "二进制 Systemd 部署");
    m.insert(
        "req.deploy.binary_desc",
        "二进制部署，Systemd 管理；轻量部署，适合定制环境",
    );
    m.insert("req.contact.label", "联系方式");
    m.insert("req.contact.wechat", "微信");
    m.insert("req.contact.email", "邮箱");
    m.insert("req.contact.telegram", "Telegram");
    m.insert("req.contact.phone", "手机");
    m.insert("req.contact.placeholder.wechat", "请输入您的微信号");
    m.insert("req.contact.placeholder.email", "请输入您的邮箱");
    m.insert("req.contact.placeholder.telegram", "请输入您的 Telegram");
    m.insert("req.contact.placeholder.phone", "请输入您的手机号");
    m.insert("req.note.label", "补充说明");
    m.insert("req.note.optional", "选填");
    m.insert(
        "req.note.placeholder",
        "请详细描述您的需求、使用场景、预算、期望目标等",
    );
    m.insert("req.submit", "提交需求");
    m.insert("req.submitting", "提交中…");
    m.insert(
        "req.privacy",
        "提交即表示同意隐私政策，我们会严格保护您的信息安全",
    );
    m.insert("req.success", "已收到，我们会尽快与您联系");
    m.insert("req.err.contact_required", "请填写联系方式");
    m.insert("req.err.contact_invalid", "联系方式格式不正确");
    m.insert("req.err.failed", "提交失败，请稍后重试");

    // ── Login ───────────────────────────────────
    m.insert("login.tagline_1", "新一代");
    m.insert("login.tagline_highlight", "AI Token 算力服务平台");
    m.insert("login.tagline_2", "让每一个 Token 都创造价值");
    m.insert("login.tagline_3", "");
    m.insert(
        "login.description",
        "统一大模型接入、智能路由调度、实时计费结算与全链路可观测性。开箱即用的企业级 AI Token 算力服务平台。",
    );
    m.insert("login.feature_routing", "智能路由");
    m.insert("login.feature_billing", "实时计费");
    m.insert("login.feature_ha", "节点租赁");
    m.insert("login.feature_api", "分布式集群");
    m.insert("login.feature_metering", "精准计量计费");
    m.insert("login.feature_custom", "需求定制化");
    m.insert("login.title", "登录");
    m.insert("login.subtitle", "管理您的 AI Token 与算力资源");
    m.insert("login.email_label", "邮箱地址");
    m.insert("login.hide_password", "隐藏密码");
    m.insert("login.show_password", "显示密码");
    m.insert("login.verifying", "验证中...");
    m.insert("login.submit", "登录到控制台");
    m.insert("reset_password.failed", "重置失败");
    m.insert("reset_password.success", "密码已重置成功！");
    m.insert("reset_password.go_login", "前往登录");
    m.insert("reset_password.submit", "确认重置");

    // ── Account Settings ────────────────────────
    m.insert("account_settings.subtitle", "更新登录密码与账户安全信息");
    m.insert("account_settings.fill_all_passwords", "请填写所有密码字段");
    m.insert("account_settings.password_mismatch", "两次新密码输入不一致");
    m.insert("account_settings.password_too_short", "新密码至少需要 8 位");
    m.insert(
        "account_settings.password_no_uppercase",
        "新密码需包含至少一个大写字母",
    );
    m.insert(
        "account_settings.password_no_lowercase",
        "新密码需包含至少一个小写字母",
    );
    m.insert(
        "account_settings.password_no_digit",
        "新密码需包含至少一个数字",
    );
    m.insert(
        "account_settings.password_no_special",
        "新密码需包含至少一个特殊字符",
    );
    m.insert("account_settings.password_changed", "密码修改成功");
    m.insert("account_settings.change_failed", "修改失败");
    m.insert("account_settings.change_password", "修改密码");
    m.insert(
        "account_settings.section_desc",
        "当前页专注于账户安全操作。输入区保持窄栏，避免在右侧无内容时被直接拉满。",
    );
    m.insert("account_settings.current_password", "当前密码");
    m.insert(
        "account_settings.current_password_desc",
        "用于确认本次操作来自当前登录账户。",
    );
    m.insert(
        "account_settings.current_password_placeholder",
        "请输入当前密码",
    );
    m.insert("account_settings.new_password", "新密码");
    m.insert(
        "account_settings.new_password_desc",
        "建议使用长度更高、包含大小写和符号的强密码。",
    );
    m.insert(
        "account_settings.new_password_placeholder",
        "请输入新密码（至少8位）",
    );
    m.insert("account_settings.confirm_password", "确认新密码");
    m.insert(
        "account_settings.confirm_password_desc",
        "再次输入相同密码，避免误录导致锁定登录。",
    );
    m.insert(
        "account_settings.confirm_password_placeholder",
        "再次输入新密码",
    );

    // ── Profile ─────────────────────────────────
    m.insert("profile.saved", "保存成功");
    m.insert("profile.save_failed", "保存失败");
    m.insert(
        "profile.page_desc",
        "查看当前账户身份信息，并维护控制台显示名称。",
    );
    m.insert("profile.tenant", "租户");
    m.insert("profile.user_id", "用户 ID");
    m.insert("profile.edit", "编辑资料");

    // ── Usage ───────────────────────────────────
    m.insert("usage.subtitle", "查看 API 调用记录与 Token 消耗");
    m.insert("usage.calls", "调用次数");
    m.insert("usage.total_calls", "总调用次数");
    m.insert("usage.period", "统计周期");
    m.insert("usage.total_tokens", "总 Token 数");
    m.insert("usage.prompt_tokens", "提示词");
    m.insert("usage.completion_tokens", "补全");
    m.insert("usage.total_cost", "累计费用");
    m.insert("usage.status_success", "成功");
    m.insert("usage.status_failed", "失败");
    m.insert("usage.usage_billed", "按使用量计费");
    m.insert("usage.trend", "调用趋势");
    m.insert("usage.records", "调用记录");
    m.insert("usage.no_records", "暂无记录");
    m.insert("usage.model", "模型");
    m.insert("usage.total_token", "总 Token");

    // ── Payments ────────────────────────────────
    m.insert("payments.title", "支付与账单");
    m.insert("payments.subtitle", "查看账户余额、充值记录与账单明细");
    m.insert("payments.recharge_now", "立即充值");
    m.insert("payments.account_balance", "账户余额");
    m.insert("payments.frozen_amount", "冻结金额");
    m.insert("payments.total_recharge", "总充值");
    m.insert("payments.total_consumed", "总消耗");
    m.insert("payments.usage_requests", "用量请求数");
    m.insert("payments.input_tokens", "输入 Tokens");
    m.insert("payments.output_tokens", "输出 Tokens");
    m.insert("payments.total_tokens", "总 Tokens");
    m.insert("payments.total_cost", "总费用");
    m.insert("payments.recharge_records", "充值记录");
    m.insert("payments.no_recharge_records", "暂无充值记录");
    m.insert("payments.order_no", "订单号");
    m.insert("payments.subject", "主题");

    // ── Payment Orders ──────────────────────────
    m.insert(
        "payment_orders.subtitle_admin",
        "查看和管理平台所有支付订单",
    );
    m.insert("payment_orders.subtitle_user", "查看您的充值和支付记录");
    m.insert("payment_orders.empty", "暂无订单记录");
    m.insert("payment_orders.col_user", "用户");
    m.insert("payment_orders.provider_switch", "运营开关");
    m.insert("payment_orders.provider_config", "配置状态");
    m.insert("payment_orders.verify_provider", "验证渠道配置");
    m.insert("payment_orders.verifying_provider", "验证中...");
    m.insert("common.configured", "已配置");
    m.insert("common.not_configured", "未配置");
    m.insert("payment_orders.pagination", "共 {total} 条");
    m.insert("payment_orders.filter_all", "全部");
    m.insert("payment_orders.filter_pending", "待支付");
    m.insert("payment_orders.filter_paid", "已支付");
    m.insert("payment_orders.filter_failed", "已失败");
    m.insert("payment_orders.filter_closed", "已关闭");
    m.insert("payment_orders.status_pending", "待支付");
    m.insert("payment_orders.status_processing", "处理中");
    m.insert("payment_orders.status_paid", "已支付");
    m.insert("payment_orders.status_failed", "已失败");
    m.insert("payment_orders.status_closed", "已关闭");
    m.insert("payment_orders.status_cancelled", "已取消");
    m.insert("payment_orders.provider_available", "可用");
    m.insert("payment_orders.provider_disabled", "已停用");
    m.insert("payment_orders.provider_misconfigured", "配置不完整");
    m.insert("payment_orders.provider_state_error", "状态异常");
    m.insert("payment_orders.provider_unverified", "待验证");
    m.insert("payment_orders.provider_unavailable", "不可用");
    m.insert("payment_orders.provider_degraded", "性能下降");
    m.insert(
        "payment_orders.provider_misconfigured_message",
        "渠道已启用，但密钥或回调配置不完整。",
    );
    m.insert(
        "payment_orders.provider_state_error_message",
        "渠道状态不可用或无效，已暂停接受新订单。",
    );
    m.insert(
        "payment_orders.provider_unverified_message",
        "配置已加载，完成真实渠道验证后才会投入使用。",
    );
    m.insert(
        "payment_orders.provider_unavailable_message",
        "渠道认证或验签失败，已暂停接受新订单。",
    );
    m.insert(
        "payment_orders.provider_degraded_message",
        "渠道近期请求不稳定，当前仍接受新订单并持续观察。",
    );

    // ── Recharge ──────────────────────────────
    m.insert("recharge.title", "账户充值");
    m.insert("recharge.select_method", "选择充值方式");
    m.insert("recharge.payment_method", "支付方式");
    m.insert("recharge.alipay", "支付宝");
    m.insert("recharge.wechat_pay", "微信支付");
    m.insert(
        "recharge.no_payment_methods",
        "当前暂无可用支付方式，请稍后再试或联系管理员",
    );
    m.insert("recharge.amount_label", "充值金额（元）");
    m.insert("recharge.custom_amount", "或输入自定义金额");
    m.insert("recharge.creating_order", "订单创建中…");
    m.insert("recharge.confirm_recharge", "确认充值");
    m.insert(
        "recharge.hint",
        "充值完成后余额通常几秒内到账。若长时间未到账请联系客服。",
    );
    m.insert("recharge.pay_title", "请完成支付");
    m.insert("recharge.order_created", "订单已创建，等待支付");
    m.insert("recharge.order_no_label", "订单号：");
    m.insert("recharge.open_payment", "打开支付页面");
    m.insert(
        "recharge.refresh_hint",
        "支付完成后点击【已完成支付】按钮刷新状态",
    );
    m.insert(
        "recharge.pay_alipay_page",
        "💳 请打开支付宝支付页面完成付款",
    );
    m.insert("recharge.pay_wap", "📱 请在手机端打开支付链接完成付款");
    m.insert("recharge.pay_other", "请点击下方按钮完成支付");
    m.insert("recharge.scan_pay", "请使用支付宝或其他扫码工具完成支付");
    m.insert("recharge.scan_wechat", "请使用微信扫描二维码完成支付");
    m.insert("recharge.scan_alipay", "请使用支付宝扫描二维码完成支付");
    m.insert("recharge.qr_code_alt", "支付二维码");
    m.insert("recharge.qr_code_content", "二维码内容：");
    m.insert("recharge.confirm_paid", "已完成支付，点此确认");
    m.insert("recharge.pay_later", "稍后支付");
    m.insert("recharge.success_title", "充值成功！");
    m.insert("recharge.success_desc", "余额已入账，可立即使用 API");
    m.insert("recharge.view_balance", "查看余额");
    m.insert("recharge.continue_recharge", "继续充值");
    m.insert("recharge.enter_amount", "请输入充值金额");
    m.insert("recharge.invalid_amount", "请输入有效金额（大于 0）");
    m.insert("recharge.pay_success", "支付成功！余额将尽快入账");
    m.insert("recharge.pay_success_credited", "支付成功！余额已入账");
    m.insert("recharge.order_status", "订单状态：{status}");
    m.insert("recharge.order_expired", "订单已失效：{status}");
    m.insert("recharge.create_failed", "创建订单失败：{error}");
    m.insert("recharge.account_recharge_subject", "账户充值");
    m.insert(
        "recharge.recharge_amount_format",
        "{site_name} 账户充值 {amount} 元",
    );

    // ── Distribution ────────────────────────────
    m.insert("distribution.title", "分销管理");
    m.insert("distribution.subtitle", "查看您的分销收益和推荐记录");
    m.insert("distribution.fetch_failed", "获取失败");
    m.insert("distribution.disabled_title", "分销功能未启用");
    m.insert(
        "distribution.disabled_desc",
        "当前系统尚未开启分销功能，因此暂时无法查看推荐收益、邀请码和推荐用户信息。",
    );
    m.insert("distribution.total_earnings", "总收益");
    m.insert("distribution.available_balance", "可用余额");
    m.insert("distribution.pending", "待结算");
    m.insert("distribution.status_settled", "已结算");
    m.insert("distribution.status_pending", "待结算");
    m.insert("distribution.status_cancelled", "已取消");
    m.insert("distribution.status_failed", "失败");
    m.insert("distribution.referral_count", "推荐人数");
    m.insert("distribution.my_invite_link", "我的邀请链接");
    m.insert("distribution.referral_users", "推荐用户");
    m.insert("distribution.user", "用户");
    m.insert("distribution.joined_at", "加入时间");
    m.insert("distribution.total_spent", "消费总额");
    m.insert("distribution.my_earnings", "我的收益");
    m.insert("distribution.no_referrals", "暂无推荐记录");
    m.insert("distribution.disabled_message", "分销功能当前未开启");

    // ── Settings ────────────────────────────────
    m.insert(
        "settings.admin_desc",
        "按运行策略统一管理平台参数，保持控制台配置紧凑、清晰、可审阅。",
    );
    m.insert(
        "settings.user_desc",
        "查看系统运行配置（仅供参考）。仅管理员可以修改全局参数。",
    );
    m.insert(
        "settings.admin_only_hint",
        "系统设置仅 Admin 可修改。个人语言与主题偏好请通过顶部导航栏右侧按钮切换。",
    );
    m.insert("settings.load_failed", "设置加载失败");
    m.insert("settings.saved", "设置已保存");
    m.insert("settings.basic_title", "基础配置");
    m.insert(
        "settings.basic_desc",
        "定义平台名称、新用户赠送额度和充值基础参数。界面刻意保持窄栏，避免输入区在宽屏下失控铺开。",
    );
    m.insert("settings.site_name_label", "平台名称");
    m.insert(
        "settings.site_name_desc",
        "显示在登录页、后台导航和邮件模板中的平台名称。",
    );
    m.insert("settings.default_user_quota_label", "新用户默认赠送额度");
    m.insert(
        "settings.default_user_quota_desc",
        "运行时按此值决定新用户注册赠送额度；只有大于 0 才会赠送，0 或负数表示不赠送。",
    );
    m.insert("settings.default_currency_label", "默认货币");
    m.insert(
        "settings.default_currency_desc",
        "影响后台金额展示、订单默认币种和部分前端文案。",
    );
    m.insert("settings.min_recharge_label", "最低充值金额");
    m.insert("settings.max_recharge_label", "最高充值金额");
    m.insert("settings.max_recharge_desc", "限制单笔充值金额上限。");
    m.insert("settings.payment_title", "支付渠道");
    m.insert(
        "settings.payment_desc",
        "运营开关只控制新订单；配置不完整的渠道不会向用户展示。",
    );
    m.insert(
        "settings.alipay_enabled_desc",
        "允许支付宝创建新的充值订单。",
    );
    m.insert(
        "settings.wechatpay_enabled_desc",
        "允许微信 Native 支付创建新的充值订单。",
    );
    m.insert(
        "settings.min_recharge_desc",
        "限制单次充值的最低金额，避免异常小额订单进入支付链路。",
    );
    m.insert("settings.security_title", "安全配置");
    m.insert(
        "settings.security_desc",
        "控制令牌有效期等安全参数。新用户注册始终强制邮箱验证码验证。",
    );
    m.insert("settings.jwt_expire_label", "JWT Token 有效期（小时）");
    m.insert(
        "settings.jwt_expire_desc",
        "登录后访问令牌的默认有效期。时间越长，体验更顺滑，但凭证暴露窗口也更大。",
    );
    m.insert("settings.save_failed", "保存失败");
    m.insert("settings.non_negative", "值不能为负数");
    m.insert("settings.invalid_number", "请输入有效的数字");
    m.insert("settings.distribution_title", "分销开关");
    m.insert(
        "settings.distribution_desc",
        "分销功能只保留一个全局开关，由 system 角色统一控制。",
    );
    m.insert("settings.distribution_enabled_label", "启用分销功能");
    m.insert(
        "settings.distribution_enabled_desc",
        "开启后用户可访问分销中心及推荐相关接口；关闭后相关接口会返回禁用状态。",
    );
    m.insert(
        "settings.distribution_enabled_system_only_desc",
        "当前状态仅供查看。只有 system 角色可以在后台修改分销开关。",
    );

    // ── Pricing ─────────────────────────────────
    m.insert("pricing.admin_desc", "管理平台定价策略，设置模型调用费率");
    m.insert("pricing.user_desc", "查看当前平台可用的定价策略");
    m.insert("pricing.create", "+ 新建定价");
    m.insert("pricing.empty", "暂无定价策略");
    m.insert("pricing.table_title", "模型定价表");
    m.insert(
        "pricing.table_subtitle",
        "统一查看各模型的 Provider 归属、输入输出费率和默认策略，保证计费配置可审阅且易于对比。",
    );
    m.insert("pricing.items_suffix", "条");
    m.insert("pricing.model_provider", "模型 / Provider");
    m.insert("pricing.tenant_id", "租户 ID");
    m.insert("pricing.global", "全局默认");
    m.insert("pricing.input_price", "输入价格");
    m.insert("pricing.output_price", "输出价格");
    m.insert("pricing.billing_status", "计费状态");
    m.insert("pricing.input_tokens", "input tokens");
    m.insert("pricing.output_tokens", "output tokens");
    m.insert("pricing.default", "默认");
    m.insert("pricing.alternative", "备选");
    m.insert("pricing.default_note", "当前模型计费默认落在这条规则");
    m.insert("pricing.alternative_note", "未设为默认，需手动切换后生效");
    m.insert("pricing.set_default_ok", "已设为默认定价");
    m.insert("pricing.set_default_failed", "设置默认失败");
    m.insert("pricing.set_default", "设为默认");
    m.insert("pricing.deleted", "定价已删除");
    m.insert("pricing.delete_failed", "删除失败");
    m.insert("pricing.delete_confirm_title", "删除定价");
    m.insert(
        "pricing.delete_confirm_message",
        "确定删除模型“{model}”的这条定价吗？删除后无法恢复。",
    );
    m.insert("pricing.created", "定价创建成功");
    m.insert("pricing.updated", "定价更新成功");
    m.insert("pricing.fill_all", "请填写所有字段");
    m.insert("pricing.invalid_input_price", "输入单价格式不正确");
    m.insert("pricing.invalid_output_price", "输出单价格式不正确");
    m.insert("pricing.negative_input_price", "输入单价不能为负数");
    m.insert("pricing.negative_output_price", "输出单价不能为负数");
    m.insert("pricing.create_failed", "创建失败");
    m.insert("pricing.update_failed", "更新失败");
    m.insert("pricing.create_title", "新建定价");
    m.insert("pricing.edit_title", "编辑定价");
    m.insert("pricing.model_name", "模型名称");
    m.insert("pricing.model_placeholder", "如 gpt-4o");
    m.insert("pricing.provider_type", "计费维度");
    m.insert("pricing.provider_type_placeholder", "选择计费维度");
    m.insert("pricing.label_provider_account", "Provider 账号");
    m.insert("pricing.label_node", "节点");
    m.insert("pricing.input_price_label", "输入单价（每1K tokens）");
    m.insert("pricing.output_price_label", "输出单价（每1K tokens）");
    m.insert("pricing.input_placeholder", "如 0.000005");
    m.insert("pricing.output_placeholder", "如 0.000015");
    m.insert("pricing.currency_cny", "CNY（人民币）");
    m.insert("pricing.currency_usd", "USD（美元）");
    m.insert("pricing.creating", "创建中...");

    // ── Dashboard ───────────────────────────────
    m.insert(
        "dashboard.subtitle_long",
        "这是您的控制台概览，下面是当前账户的实时指标、最近活动与关键操作入口。",
    );
    m.insert("dashboard.balance_available", "可用余额");
    m.insert("dashboard.total_cost", "累计费用");
    m.insert("dashboard.meta_usage", "来自真实用量聚合");
    m.insert("dashboard.meta_balance", "账户余额实时返回");
    m.insert("dashboard.meta_keys", "当前启用中的密钥");
    m.insert("dashboard.meta_cost", "真实 usage_logs 聚合");
    m.insert("dashboard.recent_active_days", "最近 7 个活跃日");
    m.insert(
        "dashboard.recent_active_days_desc",
        "按真实请求记录聚合，快速判断近期活跃度变化。",
    );
    m.insert("dashboard.live_data", "实时数据");
    m.insert(
        "dashboard.quick_links_desc",
        "围绕充值、密钥和账户操作组织控制台主路径。",
    );
    m.insert("dashboard.manage_api_keys_desc", "创建、查看与吊销访问密钥");
    m.insert("dashboard.payments", "支付与账单");
    m.insert("dashboard.payments_desc", "查看余额、充值记录和订单状态");
    m.insert("dashboard.usage_details", "用量明细");
    m.insert(
        "dashboard.usage_details_desc",
        "审阅模型调用、Tokens 与费用",
    );
    m.insert("dashboard.account_settings_desc", "更新个人资料与安全信息");
    m.insert("dashboard.recent_calls", "最近调用");
    m.insert(
        "dashboard.recent_calls_desc",
        "使用真实 usage 记录作为控制台活动流。",
    );
    m.insert("dashboard.no_recent_calls", "暂无最近调用记录。");
    m.insert("dashboard.active_keys_panel", "活跃密钥");
    m.insert(
        "dashboard.active_keys_panel_desc",
        "只展示仍在启用状态的 Key。",
    );
    m.insert("dashboard.no_active_keys", "暂无活跃密钥。");
    m.insert("dashboard.system_status", "系统状态");
    m.insert("dashboard.account_status", "账户状态");
    m.insert(
        "dashboard.system_status_desc",
        "管理员可见的网关与 Provider 健康摘要。",
    );
    m.insert(
        "dashboard.account_status_desc",
        "围绕余额、分销和订单状态汇总当前账户。",
    );
    m.insert("dashboard.online", "在线");
    m.insert("dashboard.pending_check", "待检查");
    m.insert("dashboard.gateway_providers", "网关 Provider");
    m.insert("dashboard.gateway_providers_desc", "已加载 Provider 数量");
    m.insert("dashboard.healthy_providers", "健康 Provider");
    m.insert("dashboard.healthy_providers_desc", "当前健康的路由目标");
    m.insert("dashboard.account_cache", "渠道状态缓存");
    m.insert("dashboard.account_cache_desc", "账号状态存储中的条目数");
    m.insert("dashboard.fallback_count", "Fallback 次数");
    m.insert("dashboard.fallback_count_desc", "来自真实网关统计");
    m.insert("dashboard.total_distribution_earnings", "总分销收益");
    m.insert("dashboard.total_distribution_earnings_desc", "累计推荐收益");
    m.insert("dashboard.pending_distribution_earnings", "待结算收益");
    m.insert(
        "dashboard.pending_distribution_earnings_desc",
        "尚未结算到可提现金额",
    );
    m.insert("dashboard.referral_count_desc", "当前已绑定推荐关系");
    m.insert("dashboard.latest_order", "最近订单");
    m.insert("dashboard.latest_order_desc", "最近一笔充值订单状态");
    m.insert("dashboard.none", "暂无");
    m.insert("dashboard.last_used_prefix", "最近使用");
    m.insert("dashboard.no_usage_record", "暂无使用记录");
    m.insert("system.provider_health", "Provider 健康状态");
    m.insert("system.no_healthy_provider", "当前没有健康 Provider");
    m.insert("system.gateway_stats", "网关运行统计");
    m.insert("system.total_requests", "总请求数");
    m.insert("system.success_rate", "成功率");
    m.insert("system.avg_latency", "平均响应时间");
    m.insert("system.fallback_count", "Fallback 次数");
    m.insert("system.routing_debug", "路由调试");
    m.insert("system.provider_status_diagnosis", "Provider 状态诊断");
    m.insert("system.route_success", "路由成功");
    m.insert("system.primary_target", "主目标");
    m.insert("system.fallback_chain", "备用链路");
    m.insert("system.items", "个");
    m.insert("system.route_failed", "路由失败");
    m.insert(
        "system.no_routable_probe_model",
        "当前租户没有可用于 Provider 路由探测的启用账号模型",
    );
    m.insert("system.provider_status", "Provider 状态");
    m.insert("system.no_provider_configured", "未配置任何 Provider");
    m.insert("system.provider_column", "Provider");
    m.insert("system.health_status", "健康状态");
    m.insert("system.account_count", "账号数量");
    m.insert("system.healthy", "健康");
    m.insert("system.unhealthy", "不健康");
    m.insert("system.pricing_info", "定价信息");

    m.insert(
        "node_gateway.subtitle",
        "管理本地节点接入、任务队列和 node: 模型执行路径。",
    );
    m.insert("node_gateway.runtime_status", "运行状态");
    m.insert(
        "node_gateway.runtime_desc",
        "Node Gateway 依赖 Redis 队列、Postgres 状态表和节点会话 token。",
    );
    m.insert("node_gateway.enabled", "已启用");
    m.insert("node_gateway.disabled", "未启用");
    m.insert("node_gateway.nodes_total", "节点总数");
    m.insert("node_gateway.nodes_total_desc", "已注册节点实例");
    m.insert("node_gateway.nodes_online", "在线节点");
    m.insert("node_gateway.nodes_online_desc", "可参与 node: 路由");
    m.insert("node_gateway.tasks_active", "活跃任务");
    m.insert("node_gateway.tasks_active_desc", "queued + leased");
    m.insert("node_gateway.tasks_done", "成功任务");
    m.insert("node_gateway.tasks_done_desc", "已返回结果");
    m.insert("node_gateway.protocol_title", "协议入口");
    m.insert(
        "node_gateway.protocol_register",
        "节点首次注册，使用 registration token 换取 session token。",
    );
    m.insert(
        "node_gateway.protocol_heartbeat",
        "刷新 session 可见性并上报当前可接收模型。",
    );
    m.insert("node_gateway.protocol_poll", "长轮询领取匹配模型的任务。");
    m.insert(
        "node_gateway.protocol_complete",
        "提交任务结果，支持幂等重试。",
    );
    m.insert("node_gateway.nodes_title", "节点列表");
    m.insert("node_gateway.tasks_title", "最近任务");
    m.insert("node_gateway.no_nodes", "暂无注册节点");
    m.insert("node_gateway.no_tasks", "暂无节点任务");
    m.insert("node_gateway.node", "节点");
    m.insert("node_gateway.models", "可接收模型");
    m.insert("node_gateway.failures", "失败计数");
    m.insert("node_gateway.heartbeat", "最近心跳");
    m.insert("node_gateway.no_models", "暂无模型");
    m.insert("node_gateway.assigned_node", "领取节点");
    m.insert("node_gateway.deadline", "截止时间");
    m.insert("node_gateway.status_online", "在线");
    m.insert("node_gateway.status_offline", "离线");
    m.insert("node_gateway.status_excluded", "已排除");
    m.insert("node_gateway.token_status_pending", "待审批");
    m.insert("node_gateway.task_queued", "排队中");
    m.insert("node_gateway.task_leased", "执行中");
    m.insert("node_gateway.task_succeeded", "成功");
    m.insert("node_gateway.task_failed", "失败");
    m.insert("node_gateway.task_expired", "已过期");
    // ── 令牌审批（Admin）────────────────────────
    m.insert("node_gateway.token_approval_title", "注册令牌审批");
    m.insert(
        "node_gateway.token_approval_desc",
        "审核用户提交的节点注册令牌申请",
    );
    m.insert("node_gateway.token_approval_email", "用户邮箱");
    m.insert("node_gateway.token_approval_preview", "令牌预览");
    m.insert("node_gateway.token_approval_apply_time", "申请时间");
    m.insert("node_gateway.no_pending_tokens", "暂无待审批的注册令牌申请");
    m.insert(
        "node_gateway.token_approval_pending_count",
        "{count} 条待审批",
    );
    m.insert("node_gateway.approve", "通过");
    m.insert("node_gateway.reject", "拒绝");
    m.insert("node_gateway.approve_confirm_title", "确认通过");
    m.insert(
        "node_gateway.approve_confirm_msg",
        "确认通过该用户的令牌申请？通过后用户将可以查看令牌明文并用于节点注册。",
    );
    m.insert("node_gateway.reject_confirm_title", "确认拒绝");
    m.insert(
        "node_gateway.reject_confirm_msg",
        "确认拒绝该用户的令牌申请？用户可重新申请。",
    );
    m.insert("node_gateway.approve_success", "令牌已审批通过");
    m.insert("node_gateway.reject_success", "令牌申请已拒绝");
    m.insert("node_gateway.approve_failed", "审批操作失败");
    m.insert("node_gateway.token_conflict", "该申请已被其他管理员处理");
    m.insert("node_gateway.exclude", "排除");
    m.insert("node_gateway.exclude_success", "节点已排除");
    m.insert("node_gateway.exclude_failed", "排除节点失败");
    m.insert("node_gateway.exclude_confirm_title", "确认排除节点");
    m.insert(
        "node_gateway.exclude_confirm_msg",
        "排除后该节点将不再接收任务分配，但节点仍可通过心跳保持连接。确定要排除吗？",
    );
    m.insert("node_gateway.revoke", "吊销");
    m.insert("node_gateway.revoke_success", "注册令牌已吊销");
    m.insert("node_gateway.revoke_failed", "吊销注册令牌失败");
    m.insert("node_gateway.revoke_confirm_title", "确认吊销节点");
    m.insert(
        "node_gateway.revoke_confirm_msg",
        "吊销后该节点将被排除且注册令牌作废，节点无法注册新实例。可通过恢复按钮撤销。",
    );
    m.insert("node_gateway.recover", "恢复");
    m.insert("node_gateway.recover_success", "节点已恢复上线");
    m.insert("node_gateway.recover_failed", "恢复节点失败");
    m.insert("node_gateway.recover_confirm_title", "确认恢复节点");
    m.insert(
        "node_gateway.recover_confirm_msg",
        "恢复后该节点将重新上线并可接收任务分配，连续失败计数将清零。确定要恢复吗？",
    );
    m.insert("node_gateway.token_preview", "注册令牌");
    m.insert("node_gateway.revoke_reason_label", "吊销原因");
    m.insert(
        "node_gateway.revoke_reason_placeholder",
        "请填写吊销原因...",
    );
    m.insert("node_gateway.delete_confirm_title", "确认删除节点");
    m.insert(
        "node_gateway.delete_confirm_msg",
        "删除后该节点所有数据将被彻底清除，用户的注册令牌记录将被删除，用户需重新申请。此操作不可撤销。",
    );
    m.insert("node_gateway.delete", "删除");
    m.insert("node_gateway.delete_success", "节点已删除");
    m.insert("node_gateway.delete_failed", "删除节点失败");

    m.insert(
        "monitoring.subtitle",
        "集中查看请求趋势、Provider 与节点健康、执行链路和路由诊断。",
    );
    m.insert("monitoring.views", "监控与诊断视图");
    m.insert("monitoring.overview_tab", "监控概览");
    m.insert("monitoring.diagnostics_tab", "系统诊断");
    m.insert(
        "monitoring.diagnostics_intro",
        "进程内指标反映当前网关实例且不受概览时间范围影响；路由诊断按当前租户的真实账号与入口协议检查主备链路、Provider 状态和定价结果。",
    );
    m.insert("monitoring.control_plane", "执行链路概览");
    m.insert(
        "monitoring.control_plane_desc",
        "基于现有 node_tasks、nodes、node_sessions 和 usage_logs 聚合，只读展示运行态。",
    );
    m.insert("monitoring.read_only", "只读");
    m.insert("monitoring.online_nodes", "在线节点");
    m.insert("monitoring.online_nodes_desc", "当前可参与调度");
    m.insert("monitoring.active_tasks", "活跃任务");
    m.insert("monitoring.active_tasks_desc", "排队或执行中的节点任务");
    m.insert("monitoring.succeeded_tasks", "成功任务");
    m.insert("monitoring.succeeded_tasks_desc", "已由节点返回结果");
    m.insert("monitoring.avg_latency", "平均耗时");
    m.insert("monitoring.avg_latency_desc", "queued 到 finished");
    m.insert("monitoring.flow_title", "Gateway + Node 流程");
    m.insert("monitoring.flow_gateway", "Gateway 接入");
    m.insert(
        "monitoring.flow_gateway_desc",
        "OpenAI 兼容入口识别 node: 模型并创建请求。",
    );
    m.insert("monitoring.flow_queue", "任务入队");
    m.insert(
        "monitoring.flow_queue_desc",
        "请求写入 node_tasks，等待匹配模型的节点领取。",
    );
    m.insert("monitoring.flow_node", "节点执行");
    m.insert(
        "monitoring.flow_node_desc",
        "节点通过 poll 领取 lease，执行后 complete 回传。",
    );
    m.insert("monitoring.flow_usage", "用量落账");
    m.insert(
        "monitoring.flow_usage_desc",
        "成功响应关联 usage_logs，用于账单和审计。",
    );
    m.insert("monitoring.health_title", "节点健康");
    m.insert("monitoring.no_nodes", "暂无节点健康数据");
    m.insert("monitoring.active", "活跃");
    m.insert("monitoring.succeeded", "成功");
    m.insert("monitoring.failed", "失败");
    m.insert("monitoring.traces_title", "最近追踪");
    m.insert("monitoring.no_traces", "暂无 gateway/node 追踪记录");
    m.insert("monitoring.records_title", "追踪明细");
    m.insert("monitoring.request", "请求");
    m.insert("monitoring.request_payload", "请求信息");
    m.insert("monitoring.basic_info", "基本信息");
    m.insert("monitoring.request_metrics", "请求指标");
    m.insert("monitoring.node", "节点");
    m.insert("monitoring.duration", "耗时");
    m.insert("monitoring.tokens", "Tokens");
    m.insert("monitoring.queued_at", "入队时间");
    m.insert("monitoring.task", "任务");
    m.insert("monitoring.lease", "租约");
    m.insert("monitoring.stage_queued", "入队");
    m.insert("monitoring.stage_claimed", "领取");
    m.insert("monitoring.stage_finished", "完成");
    m.insert("monitoring.stage_usage", "用量");
    m.insert("monitoring.submissions", "提交");
    m.insert("monitoring.amount", "金额");
    m.insert("monitoring.total_usage_logs", "用量日志");
    m.insert("monitoring.total_node_tasks", "节点任务");
    m.insert("monitoring.failed_tasks", "异常任务");
    m.insert("monitoring.map_receive_request", "接收请求");
    m.insert("monitoring.map_return_client", "返回客户端");
    m.insert("monitoring.map_router", "路由模块");
    m.insert("monitoring.map_match_route", "匹配路由规则");
    m.insert("monitoring.map_process_request", "处理请求");
    m.insert("monitoring.map_submit_result", "提交结果");
    m.insert("monitoring.map_model_service", "模型服务");
    m.insert("monitoring.map_model_response", "模型响应");
    m.insert("monitoring.map_gateway", "网关");
    m.insert("monitoring.map_gateway_subtitle", "OpenAI API");
    m.insert("monitoring.map_router_subtitle", "节点: 模型");
    m.insert("monitoring.map_node", "节点");
    m.insert("monitoring.time_range", "时间范围");
    m.insert("monitoring.range_1h", "最近 1 小时");
    m.insert("monitoring.range_6h", "最近 6 小时");
    m.insert("monitoring.range_24h", "最近 24 小时");
    m.insert("monitoring.range_custom", "自定义 UTC");
    m.insert("monitoring.utc_from", "UTC 起始时间");
    m.insert("monitoring.utc_to", "UTC 结束时间");
    m.insert("monitoring.utc_to_title", "UTC 结束时间（最长 24 小时）");
    m.insert("monitoring.status_filter", "状态筛选");
    m.insert("monitoring.all_statuses", "全部状态");
    m.insert("monitoring.timed_out", "超时");
    m.insert("monitoring.running", "运行中");
    m.insert("monitoring.queued", "排队");
    m.insert("monitoring.routing", "路由中");
    m.insert("monitoring.cancelled", "已取消");
    m.insert("monitoring.received", "已接收");
    m.insert("monitoring.online", "在线");
    m.insert("monitoring.offline", "离线");
    m.insert("monitoring.route_filter", "执行路径筛选");
    m.insert("monitoring.all_routes", "全部路径");
    m.insert("monitoring.resume_auto_refresh", "继续自动刷新");
    m.insert("monitoring.pause_auto_refresh", "暂停自动刷新");
    m.insert("monitoring.refresh_now", "立即刷新");
    m.insert("monitoring.probe_in_progress", "探测中…");
    m.insert("monitoring.probe_done", "探测完成");
    m.insert("monitoring.probe_failed", "探测失败");
    m.insert("monitoring.probe_all_accounts", "探测全部账号");
    m.insert("monitoring.probe_confirm_title", "确认探测全部账号");
    m.insert(
        "monitoring.probe_confirm_message",
        "该操作会向所有已配置账号发起真实上游探测，可能产生少量费用。是否继续？",
    );
    m.insert("monitoring.last_updated", "最后更新");
    m.insert("monitoring.empty_range", "当前时间范围无流量");
    m.insert("monitoring.empty_filtered", "筛选条件无匹配结果");
    m.insert("monitoring.request_list", "监控请求列表");
    m.insert("monitoring.time", "时间");
    m.insert("monitoring.request_id", "Request ID");
    m.insert("monitoring.protocol_model", "协议 / 模型");
    m.insert("monitoring.execution_route", "执行路径");
    m.insert("monitoring.route_provider_account", "Provider 账号");
    m.insert("monitoring.route_node", "节点");
    m.insert("monitoring.status", "状态");
    m.insert("monitoring.duration_ttft", "总耗时 / TTFT");
    m.insert("monitoring.next_page", "下一页");
    m.insert("monitoring.request_detail", "请求详情");
    m.insert("monitoring.tenant", "租户");
    m.insert("monitoring.user", "用户");
    m.insert("monitoring.key", "Key");
    m.insert("monitoring.billing", "计费");
    m.insert("monitoring.trace_quality", "数据质量");
    m.insert("monitoring.client_first_content", "客户端首内容");
    m.insert("monitoring.not_collected", "未采集");
    m.insert("monitoring.none", "无");
    m.insert("monitoring.billing_summary", "计费摘要");
    m.insert("monitoring.detail_load_failed", "详情加载失败");
    m.insert("monitoring.attempts", "Attempts");
    m.insert(
        "monitoring.node_task_submissions",
        "Node Task / Submissions",
    );
    m.insert("monitoring.request_count", "请求量");
    m.insert("monitoring.active_queued", "活跃 {active} / 排队 {queued}");
    m.insert("monitoring.success_rate", "成功率");
    m.insert("monitoring.error_rate_value", "错误率 {rate}");
    m.insert("monitoring.attempt_success_rate", "Attempt 成功率");
    m.insert("monitoring.attempt_count", "{count} 次 attempts");
    m.insert("monitoring.fallback_rate", "Fallback 率");
    m.insert("monitoring.request_count_meta", "{count} 次请求");
    m.insert("monitoring.total_duration_percentiles", "总耗时 P50 / P95");
    m.insert("monitoring.provider_ttft", "Provider 首内容耗时");
    m.insert("monitoring.p50_p95", "P50 / P95");
    m.insert("monitoring.node_queue_execution", "Node 排队 / 执行");
    m.insert("monitoring.node_no_ttft", "P50（Node 不展示 TTFT）");
    m.insert("monitoring.tokens_amount", "Tokens / 金额");
    m.insert("monitoring.trends", "趋势");
    m.insert("monitoring.no_trends", "暂无趋势数据");
    m.insert("monitoring.utc_time", "UTC 时间");
    m.insert("monitoring.unrouted", "未路由");
    m.insert("monitoring.unassigned_queue", "未分配队列");
    m.insert("monitoring.view_request", "查看请求 {request_id}");
    m.insert("monitoring.provider_health", "Provider Account 健康");
    m.insert("monitoring.account_probe_link", "账号管理 / 单账号探测");
    m.insert("monitoring.no_accounts", "暂无账号");
    m.insert("monitoring.node_health", "Node 健康");
    m.insert("monitoring.node_gateway_link", "打开 Node Gateway 管理");
    m.insert("monitoring.attempt_target", "目标：{target}");
    m.insert(
        "monitoring.attempt_timing",
        "开始：{start} · 结束：{end} · 流结束：{reason}",
    );
    m.insert(
        "monitoring.attempt_http",
        "HTTP：{http} · Upstream Request ID：{upstream}",
    );
    m.insert(
        "monitoring.attempt_provider_timing",
        "响应头：{headers} · 首有效内容：{first} · TTFT：{ttft}",
    );
    m.insert("monitoring.error", "错误");
    m.insert("monitoring.enabled", "已启用");
    m.insert("monitoring.disabled", "已停用");
    m.insert(
        "monitoring.provider_health_line",
        "真实成功率 {success} · 延迟 {latency}",
    );
    m.insert(
        "monitoring.provider_probe_line",
        "探测：{probe}（{at}，{latency}，错误 {error}）· 可归责失败 {failures}",
    );
    m.insert(
        "monitoring.node_counts",
        "排队 {queued} / 运行 {running} / 成功 {succeeded} / 失败 {failed} / 过期 {expired}",
    );
    m.insert(
        "monitoring.node_runtime",
        "心跳 {heartbeat} · 会话到期 {session} · 模型 {models}",
    );
    m.insert("monitoring.quality_derived", "历史推断");
    m.insert("monitoring.quality_partial", "信息不完整");
    m.insert("monitoring.quality_actual", "实际采集");
    m.insert("users.subtitle", "查看和管理平台所有注册用户");
    m.insert("users.search_placeholder", "搜索邮箱或用户名...");
    m.insert("users.empty", "暂无用户数据");
    m.insert("users.user", "用户");
    m.insert("users.tenant", "租户");
    m.insert("users.registered_at", "注册时间");
    m.insert("users.updated", "用户信息已更新");
    m.insert("users.update_failed", "更新失败");
    m.insert("users.deleted", "用户已删除");
    m.insert("users.delete_failed", "删除失败");
    m.insert("users.delete_self_forbidden", "不能删除自己的账户");
    m.insert(
        "users.delete_admin_forbidden",
        "仅 system 角色可删除管理员用户",
    );
    m.insert("users.edit_title", "编辑用户");
    m.insert("users.display_name", "显示名称");
    m.insert("users.display_name_placeholder", "留空则不修改");
    m.insert("users.role_user", "user（普通用户）");
    m.insert("users.role_admin", "admin（管理员）");
    m.insert("users.role_system", "system（受保护）");
    m.insert("users.delete_confirm_title", "确认删除");
    m.insert("users.delete_confirm_prefix", "确定要删除用户");
    m.insert("users.delete_confirm_suffix", "吗？此操作不可撤销。");
    m.insert("users.deleting", "删除中...");
    m.insert("users.confirm_delete", "确认删除");
    m.insert("users.self_title", "我的账户");
    m.insert("users.self_desc", "查看和管理您的个人账户信息");
    m.insert("users.account_info", "账户信息");
    m.insert("users.balance", "余额");
    m.insert("users.frozen_short", "冻");
    m.insert("users.balance_manage", "余额");
    m.insert("users.balance_title", "余额管理");
    m.insert("users.balance_available", "可用余额");
    m.insert("users.balance_frozen", "冻结余额");
    m.insert("users.balance_action", "操作类型");
    m.insert("users.balance_recharge", "充值");
    m.insert("users.balance_deduct", "扣除");
    m.insert("users.balance_freeze", "冻结");
    m.insert("users.balance_unfreeze", "解冻");
    m.insert("users.balance_amount", "金额");
    m.insert("users.balance_amount_placeholder", "请输入金额");
    m.insert("users.balance_reason", "原因");
    m.insert("users.balance_reason_placeholder", "请输入操作原因");
    m.insert("users.balance_amount_required", "请输入金额");
    m.insert("users.balance_amount_invalid", "金额必须大于 0");
    m.insert("users.balance_amount_precision", "金额最多支持两位小数");
    m.insert("users.balance_reason_required", "请输入操作原因");
    m.insert("users.balance_action_invalid", "无效的操作类型");
    m.insert("users.balance_updated", "余额操作成功");
    m.insert("users.balance_update_failed", "余额操作失败");
    m.insert("users.cannot_modify_system", "仅 system 角色可操作系统用户");
    m.insert("tenants.subtitle", "查看和管理平台所有租户信息");
    m.insert("tenants.search_placeholder", "搜索租户名称或 ID...");
    m.insert("tenants.empty", "暂无租户数据");
    m.insert("tenants.tenant_id", "租户 ID");
    m.insert("tenants.active", "活跃");
    m.insert(
        "distribution_records.admin_desc",
        "查看全平台分销收益记录，及当前生效的分销规则",
    );
    m.insert(
        "distribution_records.user_desc",
        "查看您通过邀请获得的分销收益明细",
    );
    m.insert("distribution_records.rules_title", "分销规则（只读）");
    m.insert(
        "distribution_records.rules_hint",
        "分销规则由平台运营方统一配置，如需调整请联系系统管理员。",
    );
    m.insert("distribution_records.no_rules", "当前无分销规则");
    m.insert("distribution_records.rule_name", "规则名称");
    m.insert("distribution_records.commission_rate", "分销比例");
    m.insert("distribution_records.empty_admin", "暂无分销记录");
    m.insert("distribution_records.record_id", "记录编号");
    m.insert("distribution_records.source_user_id", "来源用户 ID");
    m.insert("distribution_records.amount_spent", "消费金额");
    m.insert("distribution_records.commission_amount", "分销金额");
    m.insert("distribution_records.referrer_id", "推荐人 ID");
    m.insert("distribution_records.empty_user", "暂无推荐记录");
    m.insert("distribution_records.referred_user", "被推荐用户");
    m.insert(
        "accounts.subtitle",
        "统一维护各 Provider 渠道、模型映射与可用性状态，确保路由层始终有可审阅的账号资产池。",
    );
    m.insert("accounts.reset_failed", "重置失败");
    m.insert("accounts.fill_required", "请填写必填项");
    m.insert("accounts.created", "渠道已创建");
    m.insert("accounts.create_failed", "创建失败");
    m.insert("accounts.name_required", "渠道名称不能为空");
    m.insert("accounts.updated", "渠道已更新");
    m.insert("accounts.update_failed", "更新失败");
    m.insert("accounts.resetting", "重置中...");
    m.insert("accounts.reset_health", "重置健康状态");
    m.insert("accounts.add_channel", "+ 新增渠道");
    m.insert("accounts.empty", "暂无渠道配置，请点击“新增渠道”添加");
    m.insert("accounts.table_title", "渠道资产表");
    m.insert(
        "accounts.table_subtitle",
        "按 Provider 汇总当前账号池的可用状态、模型覆盖和速率余量。",
    );
    m.insert("accounts.channels_suffix", "个渠道");
    m.insert("accounts.channel", "渠道");
    m.insert("accounts.provider_model", "Provider / 模型");
    m.insert("accounts.runtime_status", "运行状态");
    m.insert("accounts.rate_quota", "速率配额");
    m.insert("accounts.key_preview", "密钥预览");
    m.insert("accounts.default_endpoint", "使用 Provider 默认 Endpoint");
    m.insert("accounts.no_models", "未配置模型");
    m.insert("accounts.route_ready", "可参与正常路由");
    m.insert("accounts.enabled_but_unhealthy", "已启用，但健康状态异常");
    m.insert("accounts.not_routed", "当前不参与路由调度");
    m.insert("accounts.rpm_label", "当前 RPM / 上限");
    m.insert("accounts.last_used", "最近使用");
    m.insert("accounts.no_usage_record", "暂无记录");
    m.insert("accounts.test_success", "连接测试成功");
    m.insert("accounts.test_failed", "测试失败");
    m.insert("accounts.test", "测试");
    m.insert("accounts.refresh_success", "模型列表已刷新");
    m.insert("accounts.refresh_failed", "刷新模型列表失败");
    m.insert("accounts.create_title", "新增 LLM 渠道");
    m.insert("accounts.channel_name", "渠道名称 *");
    m.insert("accounts.channel_name_placeholder", "如 OpenAI 官方");
    m.insert("accounts.provider", "Provider *");
    m.insert("accounts.provider_openai_compatible", "OpenAI 兼容");
    m.insert("accounts.provider_anthropic_compatible", "Anthropic 兼容");
    m.insert(
        "accounts.provider_deepseek_openai_compatible",
        "DeepSeek（OpenAI 兼容）",
    );
    m.insert(
        "accounts.provider_gemini_openai_compatible",
        "Google Gemini（OpenAI 兼容）",
    );
    m.insert(
        "accounts.provider_vllm_openai_compatible",
        "vLLM（OpenAI 兼容）",
    );
    m.insert(
        "accounts.provider_ollama_openai_compatible",
        "Ollama（OpenAI 兼容）",
    );
    m.insert("accounts.supported_models", "支持模型 *");
    m.insert("accounts.models_hint", "多个模型用逗号分隔");
    m.insert("accounts.api_key", "API Key *");
    m.insert("accounts.custom_base_url", "自定义 Base URL");
    m.insert("accounts.edit_title", "编辑 LLM 渠道");
    m.insert("accounts.new_api_key", "新 API Key（留空则不修改）");
    m.insert("accounts.new_api_key_placeholder", "留空不修改当前 Key");
    m.insert("accounts.custom_base_url_optional", "自定义 Base URL");
    m.insert("accounts.reset_base_url", "重置为协议默认端点");
    m.insert("accounts.enable_channel", "启用渠道");
    m.insert("accounts.global_visibility", "全局可见");
    m.insert(
        "accounts.global_visibility_hint",
        "开启后，所有租户的 API Key 均可路由到此渠道账号",
    );
    m.insert("accounts.tenant_id", "租户ID");
    m.insert("accounts.tenant_id_label", "所属租户 ID");
    m.insert(
        "accounts.tenant_id_hint",
        "更改后此渠道账号将归属到新的租户",
    );
    m.insert("accounts.tenant_id_keep", "-- 保持当前租户 --");
    m.insert("accounts.delete_confirm_title", "确认删除");
    m.insert("accounts.delete_confirm_prefix", "确定要删除渠道「");
    m.insert("accounts.delete_confirm_suffix", "」吗？该操作不可恢复。");
    m.insert("accounts.deleted", "渠道已删除");
    m.insert("accounts.delete_failed", "删除失败");
    m.insert("accounts.deleting", "删除中...");
    m.insert("accounts.confirm_delete", "确认删除");
    m.insert("accounts.no_permission_title", "暂无访问权限");
    m.insert(
        "accounts.no_permission_desc",
        "您没有访问「{resource}」的权限，请联系管理员",
    );
    m.insert(
        "accounts.models_placeholder_openai",
        "如: gpt-4o, gpt-4o-mini, gpt-4-turbo",
    );
    m.insert(
        "accounts.models_placeholder_claude",
        "如: claude-3-5-sonnet-latest, claude-3-opus-latest",
    );
    m.insert(
        "accounts.models_placeholder_deepseek",
        "如: deepseek-chat, deepseek-coder",
    );
    m.insert(
        "accounts.models_placeholder_gemini",
        "如: gemini-1.5-pro, gemini-1.5-flash",
    );
    m.insert(
        "accounts.models_placeholder_vllm",
        "输入 vLLM 支持的模型名称，多个用逗号分隔",
    );
    m.insert(
        "accounts.models_placeholder_ollama",
        "输入 Ollama 模型名称，多个用逗号分隔",
    );
    m.insert(
        "accounts.models_placeholder_default",
        "输入模型名称，多个用逗号分隔",
    );

    // ── 导航（节点分组）────────────────────────────
    m.insert("nav.group.node", "节点");
    m.insert("nav.node_token", "注册令牌");
    m.insert("nav.node_earnings", "收益管理");
    m.insert("page.node_token", "注册令牌");
    m.insert("page.node_earnings", "收益管理");

    // ── 注册令牌 ─────────────────────────────
    m.insert(
        "node_token.subtitle",
        "申请节点注册令牌，用于将自己的节点接入平台",
    );
    m.insert("node_token.title", "我的令牌");
    m.insert("node_token.empty_title", "尚未申请注册令牌");
    m.insert(
        "node_token.empty_desc",
        "申请令牌后，您可以将自己的节点注册到平台，开始赚取收益。",
    );
    m.insert("node_token.apply", "申请令牌");
    m.insert("node_token.applying", "申请中...");
    m.insert("node_token.apply_success", "申请已提交，请等待管理员审批");
    m.insert("node_token.apply_failed", "申请失败");
    m.insert(
        "node_token.already_approved",
        "您已有已审批通过的令牌。如需申请新令牌，请联系管理员吊销当前令牌。",
    );
    m.insert(
        "node_token.cannot_apply",
        "当前已有活跃令牌，请先处理现有令牌后方可重新申请",
    );
    m.insert("node_token.status_pending", "待审批");
    m.insert("node_token.status_consumed", "已使用");
    m.insert("node_token.status_approved", "已通过");
    m.insert("node_token.status_rejected", "已拒绝");
    m.insert("node_token.delete", "删除记录");
    m.insert("node_token.delete_confirm_title", "确认删除");
    m.insert(
        "node_token.delete_confirm_msg",
        "确定要删除此令牌记录吗？删除后可重新申请。",
    );
    m.insert("node_token.delete_success", "记录已删除");
    m.insert("node_token.delete_failed", "删除失败");
    m.insert("node_token.status_revoked", "已吊销");
    m.insert(
        "node_token.pending_desc",
        "您的令牌申请正在等待管理员审批，请耐心等待。",
    );
    m.insert(
        "node_token.consumed_desc",
        "该令牌已被用于节点注册。如需新令牌，请重新申请。",
    );
    m.insert(
        "node_token.consumed_hint",
        "每个用户同时只能持有一个有效令牌。",
    );
    m.insert(
        "node_token.rejected_desc",
        "您的令牌申请被拒绝，您可以重新申请或联系管理员。",
    );
    m.insert(
        "node_token.revoked_desc",
        "该注册令牌已被管理员吊销，令牌已作废。但已注册的节点可能仍在运行中。",
    );
    m.insert("node_token.preview", "令牌预览");
    m.insert("node_token.issued_at", "申请时间");
    m.insert(
        "node_token.revealed_warning",
        "此令牌已查看过，请确认您已安全保存该令牌。如果遗失，需重新申请。",
    );
    m.insert(
        "node_token.first_view_hint",
        "请立即保存此令牌！令牌仅在此处完整显示一次，后续无法再查看明文。",
    );
    m.insert(
        "node_token.no_revoke_hint",
        "令牌为一次性注册凭证，注册后自动失效，无需手动吊销。",
    );
    m.insert("node_token.registered_node", "已注册节点");
    m.insert("node_token.node_status", "节点状态");
    m.insert("node_token.last_heartbeat", "最近心跳");
    m.insert(
        "node_token.node_excluded_hint",
        "节点当前处于排除状态，已停止接收任务。如需恢复，请联系管理员。",
    );
    m.insert(
        "node_token.node_online_hint",
        "节点当前在线运行中，无需重新申请令牌。如需注册新节点，请重新申请。",
    );
    m.insert(
        "node_token.reapply_hint",
        "如需注册新的节点，请重新申请令牌。",
    );
    m.insert(
        "node_token.token_hint",
        "将令牌配置到您的节点配置文件中即可完成注册。",
    );
    m.insert("node_token.copy", "复制");
    m.insert("node_token.copied", "已复制");
    m.insert("node_token.copy_hint", "点击复制");
    m.insert("node_token.help_title", "使用说明");
    m.insert("node_token.help_1", "点击「申请令牌」提交审批请求");
    m.insert(
        "node_token.help_2",
        "等待管理员审批通过后，查看令牌明文并保存",
    );
    m.insert(
        "node_token.help_3",
        "将令牌配置到节点配置文件中的 NODE_GATEWAY_TOKEN 字段",
    );
    m.insert(
        "node_token.help_4",
        "启动节点，系统将自动完成注册并开始接单",
    );
    m.insert("node_token.view_reason", "查看原因");
    m.insert(
        "node_token.no_tokens",
        "暂无令牌记录，点击上方按钮申请第一个令牌",
    );
    m.insert("node_token.expand", "展开详情");

    // ── 收益管理 ─────────────────────────────
    m.insert(
        "node_earnings.subtitle",
        "查看节点收益、小费历史，并发起提现",
    );
    m.insert("node_earnings.pending_amount", "待提现");
    m.insert("node_earnings.pending_count", "共 {count} 笔待提现");
    m.insert("node_earnings.withdrawn_amount", "已提现");
    m.insert("node_earnings.withdrawn_meta", "累计已提现金额");
    m.insert("node_earnings.total_amount", "累计收益");
    m.insert("node_earnings.total_meta", "累计产生的小费收益");
    m.insert("node_earnings.history_title", "小费历史");
    m.insert("node_earnings.withdrawals_title", "提现记录");
    m.insert("node_earnings.no_history", "暂无小费记录");
    m.insert("node_earnings.no_withdrawals", "暂无提现记录");
    m.insert("node_earnings.col_time", "时间");
    m.insert("node_earnings.col_bill_amount", "账单金额");
    m.insert("node_earnings.col_tip_amount", "小费金额");
    m.insert("node_earnings.col_tip_ratio", "分成比例");
    m.insert("node_earnings.col_status", "状态");
    m.insert("node_earnings.col_amount", "金额");
    m.insert("node_earnings.col_method", "方式");
    m.insert("node_earnings.col_remark", "备注");
    m.insert("node_earnings.status_pending", "待审批");
    m.insert("node_earnings.status_approved", "已审批");
    m.insert("node_earnings.status_completed", "已完成");
    m.insert("node_earnings.status_rejected", "已拒绝");
    m.insert("node_earnings.withdraw_btn", "发起提现");
    m.insert("node_earnings.withdraw_title", "发起提现");
    m.insert("node_earnings.withdraw_method", "提现方式");
    m.insert("node_earnings.method_balance", "转入余额");
    m.insert(
        "node_earnings.method_balance_desc",
        "即时到账，提现后金额将直接进入您的账户余额",
    );
    m.insert("node_earnings.method_alipay", "支付宝");
    m.insert(
        "node_earnings.method_alipay_desc",
        "管理员审批通过后，将线下打款到您的支付宝账户",
    );
    m.insert("node_earnings.alipay_account", "支付宝账号");
    m.insert("node_earnings.alipay_placeholder", "请输入支付宝账号");
    m.insert("node_earnings.real_name", "真实姓名");
    m.insert(
        "node_earnings.real_name_placeholder",
        "请输入支付宝实名姓名",
    );
    m.insert(
        "node_earnings.fill_alipay",
        "使用支付宝提现需填写账号和真实姓名",
    );
    m.insert(
        "node_earnings.withdraw_hint",
        "支付宝提现需管理员审批，审批通过后将在 7 个工作日内打款。",
    );
    m.insert("node_earnings.withdraw_failed", "提现失败");
    m.insert(
        "node_earnings.withdraw_balance_success",
        "提现成功！金额已转入余额",
    );
    m.insert(
        "node_earnings.withdraw_alipay_success",
        "提现申请已提交，请等待管理员审批",
    );

    m
});
