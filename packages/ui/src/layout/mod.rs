pub mod app_shell;
mod footer;
pub mod header;
pub mod sidebar;

/// KeyCompute 的规范 GitHub 仓库地址。
pub const GITHUB_REPOSITORY_URL: &str = "https://github.com/keycompute/keycompute";

pub use app_shell::{AppShell, ThemeCtx, UiState};
pub use footer::Footer;
pub use header::{Header, UserMenuAction};
pub use sidebar::{NavIcon, NavItem, NavSection, Sidebar};
