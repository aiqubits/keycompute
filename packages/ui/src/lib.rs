//! KeyCompute 共享 UI 组件库
//!
//! # 模块结构
//! - `layout`     — 应用布局（AppShell、Sidebar、Header、Footer）
//! - `components` — 通用原子组件（Button、Input、Badge、Card、Modal、Table、Loading、Alert）
//! - `charts`     — 图表组件（LineChart、BarChart、PieChart，通过 JS 互调直接使用 ECharts）
//! - `icons`      — 内联 SVG 图标组件

pub mod charts;
pub mod components;
pub mod icons;
pub mod layout;

// Re-export 最常用的布局类型，方便外部直接 `use ui::AppShell`
pub use layout::app_shell::{ThemeCtx, UiState};
pub use layout::header::UserMenuAction;
pub use layout::{
    AppShell, Footer, GITHUB_REPOSITORY_URL, Header, NavIcon, NavItem, NavSection, Sidebar,
};

// Re-export 通用组件
pub use components::alert::{Alert, AlertVariant};
pub use components::badge::{Badge, BadgeVariant};
pub use components::button::{Button, ButtonSize, ButtonVariant};
pub use components::card::{Card, StatCard};
pub use components::input::{Input, Textarea};
pub use components::loading::{CardSkeleton, LoadingOverlay, LoadingSpinner, Skeleton};
pub use components::modal::{ConfirmModal, Modal};
pub use components::page_header::PageHeader;
pub use components::table::{Pagination, Table, TableCell, TableHead};
pub use components::toast::{Toast, ToastKind, ToastMsg};

// Re-export 图表组件及数据类型
pub use charts::bar_chart::{BarChart, BarSeriesData};
pub use charts::line_chart::{LineChart, LineSeriesData};
pub use charts::pie_chart::{PieChart, PieItem};
