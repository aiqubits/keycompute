mod en;
mod zh;

pub use en::EN;
pub use zh::ZH;

/// 语言枚举
#[derive(Clone, Copy, PartialEq, Default)]
#[allow(dead_code)]
pub enum Lang {
    #[default]
    Zh,
    En,
}

impl Lang {
    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "en" => Self::En,
            _ => Self::Zh,
        }
    }

    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Zh => "zh",
            Self::En => "en",
        }
    }
}

/// 国际化结构体，通过 `.t(key)` 获取翻译文本
#[derive(Clone, Copy)]
pub struct I18n {
    lang: Lang,
}

impl I18n {
    pub fn new(lang: Lang) -> Self {
        Self { lang }
    }

    /// 获取翻译文本，未找到 key 时返回 "?"（便于快速发现缺失 key）。
    /// 注意：引用存在性测试只能覆盖静态字面量 key，动态拼接的 key（如按变量
    /// 查表的 label）不在扫描范围内，需自行保证已注册（参见 accounts.rs 的
    /// PRESETS 与 preset_label_keys_exist_in_both_maps 测试）。
    pub fn t(&self, key: &str) -> &'static str {
        let map = match self.lang {
            Lang::Zh => &ZH,
            Lang::En => &EN,
        };
        map.get(key).copied().unwrap_or("?")
    }

    /// 获取带参数的翻译文本。
    /// 在翻译值中使用 `{key}` 作为占位符，例如：
    ///   "hello_user": "Hello, {name}!"
    /// 调用：`i18n.t_with_args("hello_user", &[("name", "Alice")]`
    pub fn t_with_args(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut s = self.t(key).to_string();
        for (k, v) in args {
            s = s.replace(&format!("{{{}}}", k), v);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::{EN, I18n, Lang, ZH};
    use std::collections::HashSet;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 在临时目录写入一组 fixture 文件并扫描，返回收集到的 key 集合。
    /// 目录名带自增序号，保证并行测试间互不干扰。
    static SCAN_FIXTURE_SEQ: AtomicUsize = AtomicUsize::new(0);
    fn scan_fixture(files: &[(&str, &str)]) -> HashSet<String> {
        let seq = SCAN_FIXTURE_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("kc_i18n_scan_{}_{}", std::process::id(), seq));
        let _ = std::fs::remove_dir_all(&tmp);
        for (rel, content) in files {
            let path = tmp.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let mut keys = HashSet::new();
        collect_t_keys(&tmp, &mut keys);
        let _ = std::fs::remove_dir_all(&tmp);
        keys
    }

    /// 扫描器必须覆盖子目录 mod.rs 中的引用（回归：旧实现跳过所有 mod.rs 会漏检）
    #[test]
    fn collect_t_keys_covers_subdir_mod_files() {
        let keys = scan_fixture(&[
            ("main.rs", "fn f() { i18n.t(\"top_key\"); }\n"),
            ("sub/mod.rs", "fn g() { i18n.t(\"sub_mod_key\"); }\n"),
            (
                "sub/other.rs",
                "fn h() { i18n.t_with_args(\"twa_key\", &[]); }\n",
            ),
        ]);

        assert!(keys.contains("top_key"), "顶层 .rs 文件的 key 应被收集");
        assert!(
            keys.contains("sub_mod_key"),
            "子目录 mod.rs 的 key 应被收集"
        );
        assert!(keys.contains("twa_key"), "t_with_args 的 key 应被收集");
    }

    /// 跨行调用：`.t_with_args(` 与 key 字面量分行时，key 位于下一行行首
    /// （回归：list.rs 中 anthropic 提示文案的调用即为该形式）
    #[test]
    fn collect_t_keys_covers_multiline_t_with_args() {
        let keys = scan_fixture(&[(
            "sub/multi.rs",
            "fn g() {\n    let _ = i18n\n        .t_with_args(\n            \"multi_line_key\",\n            &[],\n        );\n}\n",
        )]);

        assert!(
            keys.contains("multi_line_key"),
            "跨行 t_with_args 的 key 应被收集"
        );
    }

    /// 动态 key：accounts.rs 通过变量查 PRESETS 的 label key，扫描器无法识别，
    /// 这里直接对常量断言，防止动态 key 缺失导致 UI 显示 "?"
    #[test]
    fn preset_label_keys_exist_in_both_maps() {
        for (_, label_key, _, _) in crate::views::shared::accounts::PRESETS.iter().copied() {
            assert!(
                ZH.contains_key(label_key),
                "zh 缺少预设 label key: {label_key}"
            );
            assert!(
                EN.contains_key(label_key),
                "en 缺少预设 label key: {label_key}"
            );
        }
    }

    /// t_with_args 的 {key} 占位符替换在 zh/en 两种语言下都必须生效，
    /// 防止只替换一侧或占位符残留（回归：list.rs 的 anthropic 提示文案依赖该机制）
    #[test]
    fn t_with_args_replaces_placeholders_in_both_langs() {
        for lang in [Lang::Zh, Lang::En] {
            let i18n = I18n::new(lang);
            let text = i18n.t_with_args(
                "api_keys.example_note_anthropic",
                &[("model", "claude-3-7-sonnet-20250219")],
            );
            assert!(
                text.contains("claude-3-7-sonnet-20250219"),
                "占位符未被替换: {text}"
            );
            assert!(!text.contains("{model}"), "占位符残留: {text}");
        }
    }

    /// t() 对缺失 key 返回 "?"（行为契约：扫描器保证静态引用不缺失，
    /// 该返回值为动态 key 缺失时的哨兵）
    #[test]
    fn t_returns_question_mark_for_missing_key() {
        let i18n = I18n::new(Lang::Zh);
        assert_eq!(i18n.t("no.such.key"), "?");
        assert_eq!(i18n.t_with_args("no.such.key", &[("a", "b")]), "?");
    }

    /// 中英文案的 key 集合必须完全一致，防止只补一侧翻译导致 key 缺失
    #[test]
    fn zh_and_en_have_identical_keys() {
        let zh_keys: HashSet<_> = ZH.keys().collect();
        let en_keys: HashSet<_> = EN.keys().collect();

        let zh_only: Vec<_> = zh_keys.difference(&en_keys).collect();
        let en_only: Vec<_> = en_keys.difference(&zh_keys).collect();

        assert!(zh_only.is_empty(), "仅中文存在的 key: {zh_only:?}");
        assert!(en_only.is_empty(), "仅英文存在的 key: {en_only:?}");
    }

    /// 代码中 `i18n.t("...")` / `t_with_args("...")` 引用的 key 必须真实存在，
    /// 否则 `t()` 会静默返回 "?"，UI 上直接暴露占位符。
    #[test]
    fn all_i18n_references_exist_in_translations() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut referenced = HashSet::new();
        collect_t_keys(&src_dir, &mut referenced);

        let mut missing: Vec<_> = referenced
            .iter()
            .filter(|key| !ZH.contains_key(key.as_str()))
            .cloned()
            .collect();
        missing.sort();

        assert!(
            missing.is_empty(),
            "代码引用了不存在的翻译 key: {missing:?}"
        );
    }

    /// 递归扫描 src 下所有 .rs 文件，收集 `t("key")` / `t_with_args("key")`
    /// 的字面量 key（含 rsx 中 `{i18n.t(\"key\")}` 的转义形式，以及
    /// `.t_with_args(` 跨行时位于下一行行首的 key）。
    /// 已知限制：`.t(` 的跨行调用与动态拼接的 key 无法识别。
    fn collect_t_keys(dir: &Path, out: &mut HashSet<String>) {
        // 仅跳过 i18n 模块自身：测试中的 marker 字面量（如 ".t(\"")会被扫描器误匹配；
        // 其他目录的 mod.rs 里的 i18n 引用仍需纳入检查
        let i18n_mod = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/i18n/mod.rs");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_t_keys(&path, out);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            if path == i18n_mod {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let mut lines = source.lines().peekable();
            while let Some(line) = lines.next() {
                // 跳过注释行，避免把文档示例中的假 key 计入
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for marker in [".t(\"", ".t(\\\"", ".t_with_args(\"", ".t_with_args(\\\""] {
                    let mut from = 0;
                    while let Some(rel) = line[from..].find(marker) {
                        let start = from + rel + marker.len();
                        let Some(len) = line[start..].find('"') else {
                            break;
                        };
                        // 转义形式 `\"` 中反斜杠属于转义符，key 尾部会残留
                        let mut key = &line[start..start + len];
                        if let Some(stripped) = key.strip_suffix('\\') {
                            key = stripped;
                        }
                        if !key.contains('{') {
                            out.insert(key.to_string());
                        }
                        from = start + len;
                    }
                }
                // 跨行调用：`.t_with_args(` 后未跟字符串时，key 在下一行行首
                if let Some(pos) = line.rfind(".t_with_args(") {
                    let after = &line[pos + ".t_with_args(".len()..];
                    if after.contains('"') {
                        continue;
                    }
                    let Some(next_line) = lines.peek() else {
                        continue;
                    };
                    let next = next_line.trim_start();
                    let Some(rest) = next.strip_prefix("\\\"").or_else(|| next.strip_prefix('"'))
                    else {
                        continue;
                    };
                    let Some(len) = rest.find('"') else { continue };
                    let mut key = &rest[..len];
                    if let Some(stripped) = key.strip_suffix('\\') {
                        key = stripped;
                    }
                    if !key.contains('{') {
                        out.insert(key.to_string());
                    }
                }
            }
        }
    }
}
