//! 通用 UI 原子组件库
//!
//! 包含以下组件：
//! - [`Button`]   — 多变体按钮（primary / secondary / danger / ghost）
//! - [`Input`]    — 文本输入框（label / hint / error 状态）
//! - [`Textarea`] — 多行文本域
//! - [`Badge`]    — 状态徽章（success / warning / error / info / neutral）
//! - [`Card`]     — 内容卡片（含 header / body / footer 插槽）
//! - [`Modal`]    — 对话框（受控 open 状态）
//! - [`Table`]    — 数据表格容器（thead + tbody 插槽）
//! - [`LoadingSpinner`] — 旋转加载指示器
//! - [`Skeleton`] — 骨架屏占位动画
//! - [`Alert`]    — 内联提示横幅（info / success / warning / error）

pub mod alert;
pub mod badge;
pub mod button;
pub mod card;
pub mod input;
pub mod loading;
pub mod modal;
pub mod page_header;
pub mod table;
pub mod toast;

pub use alert::Alert;
pub use badge::Badge;
pub use button::Button;
pub use card::Card;
pub use input::{Input, Textarea};
pub use loading::{LoadingSpinner, Skeleton};
pub use modal::Modal;
pub use page_header::PageHeader;
pub use table::Table;
pub use toast::{Toast, ToastKind, ToastMsg};
