//! Bilingual UI strings. Language/theme are persisted by the app to settings.json.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    #[serde(rename = "zh")]
    Zh,
    #[serde(rename = "en")]
    En,
}

impl Default for Lang {
    fn default() -> Self {
        Lang::Zh
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[serde(rename = "light")]
    Light,
    #[serde(rename = "dark")]
    Dark,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::Dark
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub lang: Lang,
    #[serde(default)]
    pub theme: Theme,
}

impl Default for Settings {
    fn default() -> Self {
        Self { lang: Lang::Zh, theme: Theme::Dark }
    }
}

/// All UI text in one place so adding a language is mechanical.
pub struct Str {
    pub app_title: &'static str,
    pub title: &'static str,
    pub guarding: &'static str,
    pub total: &'static str,
    pub restarts_total: &'static str,
    pub crash_records: &'static str,
    pub note_stopped: &'static str,       // template: "{} stopped or paused"
    pub note_adopted: &'static str,       // template: "{} adopted process(es)"
    pub note_backoff: &'static str,
    pub note_crash_cluster: &'static str, // template: "{} within 20 minutes"
    pub live_active: &'static str,
    pub view_all: &'static str,
    pub showing_latest: &'static str, // template: "Showing latest {} (of total)"
    pub subtitle_running: &'static str, // template: "3 of 5 entries are running"
    pub subtitle_finding: &'static str, // template: "{name} restarted {n} times"
    pub chart_restart_trend: &'static str,
    pub chart_trend_meta: &'static str,
    pub chart_uptime: &'static str,
    pub chart_uptime_meta: &'static str,
    pub health_finding: &'static str, // template: "{name} restarted {n} times today"
    pub longest_running: &'static str, // template: "Longest running: {} "
    pub label_online: &'static str,
    pub label_crashes_24h: &'static str,
    pub label_offline: &'static str,
    pub chart_pulse: &'static str,
    pub chart_pulse_meta: &'static str,
    pub pulse_event: &'static str,
    pub pulse_cluster: &'static str,
    // icon rail / views
    pub nav_overview: &'static str,
    pub nav_processes: &'static str,
    pub nav_history: &'static str,
    pub nav_settings: &'static str,
    pub settings_title: &'static str,
    pub settings_language: &'static str,
    pub settings_theme: &'static str,
    pub settings_autostart: &'static str,
    pub settings_hint: &'static str,
    pub history_empty: &'static str,
    // menu bar
    pub menu_file: &'static str,
    pub menu_guard: &'static str,
    pub menu_view: &'static str,
    pub menu_help: &'static str,
    pub menu_add: &'static str,
    pub menu_remove: &'static str,
    pub menu_open_folder: &'static str,
    pub menu_open_log: &'static str,
    pub menu_minimize_tray: &'static str,
    pub menu_quit: &'static str,
    pub menu_pause: &'static str,
    pub menu_resume: &'static str,
    pub menu_restart_all: &'static str,
    pub menu_clear_history: &'static str,
    pub menu_lang_zh: &'static str,
    pub menu_lang_en: &'static str,
    pub menu_theme: &'static str,
    pub menu_theme_light: &'static str,
    pub menu_theme_dark: &'static str,
    pub menu_autostart: &'static str,
    pub menu_about: &'static str,
    // toolbar + body
    pub add_button: &'static str,
    pub remove_button: &'static str,
    pub pause_button: &'static str,
    pub resume_button: &'static str,
    pub autostart_on: &'static str,
    pub autostart_off: &'static str,
    pub language_label: &'static str,
    pub search_hint: &'static str,
    pub empty_title: &'static str,
    pub empty_line1: &'static str,
    pub empty_line2: &'static str,
    pub col_name: &'static str,
    pub col_status: &'static str,
    pub col_pid: &'static str,
    pub col_restarts: &'static str,
    pub col_last_exit: &'static str,
    pub running: &'static str,
    pub stopped: &'static str,
    pub paused: &'static str,
    pub crash_history: &'static str,
    pub crash_count: &'static str,
    pub crash_line: &'static str,
    pub no_crashes: &'static str,
    pub add_window_title: &'static str,
    pub tab_file: &'static str,
    pub tab_process: &'static str,
    pub label_program: &'static str,
    pub label_name: &'static str,
    pub label_args: &'static str,
    pub browse: &'static str,
    pub add_and_guard: &'static str,
    pub label_filter: &'static str,
    pub refresh: &'static str,
    pub add_adopt: &'static str,
    pub about_title: &'static str,
    pub about_body: &'static str,
    pub close: &'static str,
    pub status_bar: &'static str,
    // tray
    pub tray_show: &'static str,
    pub tray_autostart_on: &'static str,
    pub tray_autostart_off: &'static str,
    pub tray_quit: &'static str,
    // toasts
    pub toast_added: &'static str,
    pub toast_adopted: &'static str,
    pub toast_removed: &'static str,
    pub toast_auto_on: &'static str,
    pub toast_auto_off: &'static str,
    pub toast_select_first: &'static str,
    pub toast_load_fail: &'static str,
    pub toast_save_fail: &'static str,
    pub toast_add_fail: &'static str,
    pub toast_auto_fail: &'static str,
    pub toast_paused: &'static str,
    pub toast_resumed: &'static str,
    pub toast_restarted: &'static str,
    pub toast_history_cleared: &'static str,
    pub toast_config_reloaded: &'static str,
}

pub fn strings(lang: Lang) -> &'static Str {
    match lang {
        Lang::Zh => &ZH,
        Lang::En => &EN,
    }
}

static ZH: Str = Str {
    app_title: "Guardian 进程守护",
    title: "Guardian 进程守护",
    guarding: "守护中",
    total: "总计",
    restarts_total: "累计重启",
    crash_records: "崩溃记录",
    note_stopped: "{} 个已停止 / 暂停",
    note_adopted: "含 {} 个接管进程",
    note_backoff: "指数退避 1s → 30s",
    note_crash_cluster: "{} 次集中在 20 分钟内",
    live_active: "守护运行中",
    view_all: "查看全部 →",
    showing_latest: "仅显示最近 {} 条",
    subtitle_running: "{} 个正在运行 / 共 {} 个守护项",
    subtitle_finding: "{} 今日重启 {} 次",
    chart_restart_trend: "重启趋势",
    chart_trend_meta: "重启次数 · 最近 12 小时",
    chart_uptime: "运行时健康度",
    chart_uptime_meta: "最近 24 小时",
    health_finding: "{} 今日重启 {} 次，是最不稳定的守护项。",
    longest_running: "最长连续运行：{}",
    label_online: "在线",
    label_crashes_24h: "24h 崩溃",
    label_offline: "离线",
    chart_pulse: "重启脉冲",
    chart_pulse_meta: "最近 60 分钟",
    pulse_event: "重启事件",
    pulse_cluster: "异常密集",
    nav_overview: "概览",
    nav_processes: "守护进程",
    nav_history: "崩溃历史",
    nav_settings: "设置",
    settings_title: "设置",
    settings_language: "语言",
    settings_theme: "主题",
    settings_autostart: "开机自启",
    settings_hint: "修改立即生效并自动保存",
    history_empty: "暂无崩溃记录",
    menu_file: "文件",
    menu_guard: "守护",
    menu_view: "视图",
    menu_help: "帮助",
    menu_add: "添加程序…",
    menu_remove: "移除选中",
    menu_open_folder: "打开数据目录",
    menu_open_log: "打开日志文件",
    menu_minimize_tray: "最小化到托盘",
    menu_quit: "退出",
    menu_pause: "暂停守护",
    menu_resume: "恢复守护",
    menu_restart_all: "重启全部",
    menu_clear_history: "清空崩溃历史",
    menu_lang_zh: "中文",
    menu_lang_en: "English",
    menu_theme: "主题",
    menu_theme_light: "浅色主题",
    menu_theme_dark: "深色主题",
    menu_autostart: "开机自启",
    menu_about: "关于 Guardian",
    add_button: "＋ 添加程序",
    remove_button: "移除选中",
    pause_button: "⏸ 暂停",
    resume_button: "▶ 恢复",
    autostart_on: "开机自启：开",
    autostart_off: "开机自启：关",
    language_label: "语言",
    search_hint: "搜索守护项…",
    empty_title: "还没有守护任何程序",
    empty_line1: "点击「添加程序」，选择一个 exe 或从正在运行的进程中挑选。",
    empty_line2: "程序意外退出时，Guardian 会自动把它拉起来。",
    col_name: "名称",
    col_status: "状态",
    col_pid: "PID",
    col_restarts: "重启次数",
    col_last_exit: "最近退出",
    running: "运行中",
    stopped: "已停止",
    paused: "已暂停",
    crash_history: "崩溃历史",
    crash_count: "条退出记录",
    crash_line: "{t}  {p}  PID={pid}  退出码={code}  存活={up}  第{n}次重启",
    no_crashes: "暂无崩溃记录",
    add_window_title: "添加要守护的程序",
    tab_file: "选择文件",
    tab_process: "正在运行的进程",
    label_program: "程序：",
    label_name: "名称：",
    label_args: "参数：",
    browse: "浏览…",
    add_and_guard: "添加并开始守护",
    label_filter: "筛选：",
    refresh: "刷新",
    add_adopt: "添加并接管选中进程",
    about_title: "关于 Guardian",
    about_body: "Guardian 是一个轻量、开源的可视化进程守护工具。\n\n程序意外退出时自动重启，采用指数退避避免疯狂重试。\n支持中英文、系统托盘、开机自启、崩溃历史。\n\n基于 Rust + egui 构建。MIT 开源协议。",
    close: "关闭",
    status_bar: "关闭窗口将最小化到托盘，守护继续运行",
    tray_show: "显示窗口",
    tray_autostart_on: "关闭开机自启",
    tray_autostart_off: "开启开机自启",
    tray_quit: "退出",
    toast_added: "已添加，开始守护",
    toast_adopted: "已接管该进程，开始守护",
    toast_removed: "已移除守护项",
    toast_auto_on: "已开启开机自启",
    toast_auto_off: "已关闭开机自启",
    toast_select_first: "请先选择一个进程",
    toast_load_fail: "配置读取失败：{}",
    toast_save_fail: "保存失败：{}",
    toast_add_fail: "添加失败：{}",
    toast_auto_fail: "自启设置失败：{}",
    toast_paused: "守护已暂停",
    toast_resumed: "守护已恢复",
    toast_restarted: "已重启全部守护进程",
    toast_history_cleared: "崩溃历史已清空",
    toast_config_reloaded: "配置已更新，守护已重新加载",
};

static EN: Str = Str {
    app_title: "Guardian — Process Watch",
    title: "Guardian — Process Watch",
    guarding: "Guarding",
    total: "Total",
    restarts_total: "Restarts",
    crash_records: "Crashes",
    note_stopped: "{} stopped or paused",
    note_adopted: "{} adopted process(es)",
    note_backoff: "Exponential backoff 1s -> 30s",
    note_crash_cluster: "{} within 20 minutes",
    live_active: "Guarding is active",
    view_all: "View all →",
    showing_latest: "Showing latest {}",
    subtitle_running: "{} of {} entries are running",
    subtitle_finding: "{} restarted {} times today",
    chart_restart_trend: "Restart trend",
    chart_trend_meta: "Restarts · last 12 hours",
    chart_uptime: "Uptime health",
    chart_uptime_meta: "Last 24 hours",
    health_finding: "{} restarted {} times today — the least stable entry.",
    longest_running: "Longest continuous run: {}",
    label_online: "Online",
    label_crashes_24h: "Crashes 24h",
    label_offline: "Offline",
    chart_pulse: "Restart pulse",
    chart_pulse_meta: "Last 60 minutes",
    pulse_event: "restart event",
    pulse_cluster: "cluster",
    nav_overview: "Overview",
    nav_processes: "Processes",
    nav_history: "History",
    nav_settings: "Settings",
    settings_title: "Settings",
    settings_language: "Language",
    settings_theme: "Theme",
    settings_autostart: "Autostart",
    settings_hint: "Changes apply immediately and save automatically",
    history_empty: "No crashes yet",
    menu_file: "File",
    menu_guard: "Guard",
    menu_view: "View",
    menu_help: "Help",
    menu_add: "Add program...",
    menu_remove: "Remove selected",
    menu_open_folder: "Open data folder",
    menu_open_log: "Open log file",
    menu_minimize_tray: "Minimize to tray",
    menu_quit: "Quit",
    menu_pause: "Pause guarding",
    menu_resume: "Resume guarding",
    menu_restart_all: "Restart all",
    menu_clear_history: "Clear crash history",
    menu_lang_zh: "中文",
    menu_lang_en: "English",
    menu_theme: "Theme",
    menu_theme_light: "Light theme",
    menu_theme_dark: "Dark theme",
    menu_autostart: "Autostart",
    menu_about: "About Guardian",
    add_button: "+ Add program",
    remove_button: "Remove",
    pause_button: "|| Pause",
    resume_button: "> Resume",
    autostart_on: "Autostart: ON",
    autostart_off: "Autostart: OFF",
    language_label: "Language",
    search_hint: "Search entries...",
    empty_title: "Nothing is being guarded yet",
    empty_line1: "Click Add program to pick an exe or choose a running process.",
    empty_line2: "Guardian restarts a program automatically if it exits unexpectedly.",
    col_name: "Name",
    col_status: "Status",
    col_pid: "PID",
    col_restarts: "Restarts",
    col_last_exit: "Last exit",
    running: "running",
    stopped: "stopped",
    paused: "paused",
    crash_history: "Crash history",
    crash_count: "recorded exits",
    crash_line: "{t}  {p}  PID={pid}  exit={code}  uptime={up}  restart#{n}",
    no_crashes: "No crash records yet",
    add_window_title: "Add a program to guard",
    tab_file: "Choose file",
    tab_process: "Running process",
    label_program: "Program:",
    label_name: "Name:",
    label_args: "Args:",
    browse: "Browse...",
    add_and_guard: "Add and start guarding",
    label_filter: "Filter:",
    refresh: "Refresh",
    add_adopt: "Add and adopt selected process",
    about_title: "About Guardian",
    about_body: "Guardian is a lightweight, open-source visual process guardian.\n\nIt restarts programs that exit unexpectedly, using exponential backoff to avoid frantic retries.\nFeatures: 中文/English, system tray, autostart, crash history.\n\nBuilt with Rust + egui. MIT licensed.",
    close: "Close",
    status_bar: "Closing the window minimizes Guardian to the tray; guarding continues",
    tray_show: "Show window",
    tray_autostart_on: "Disable autostart",
    tray_autostart_off: "Enable autostart",
    tray_quit: "Exit",
    toast_added: "Added, now guarding",
    toast_adopted: "Adopted process, now guarding",
    toast_removed: "Guard entry removed",
    toast_auto_on: "Autostart enabled",
    toast_auto_off: "Autostart disabled",
    toast_select_first: "Select a process first",
    toast_load_fail: "Config load failed: {}",
    toast_save_fail: "Save failed: {}",
    toast_add_fail: "Add failed: {}",
    toast_auto_fail: "Autostart failed: {}",
    toast_paused: "Guarding paused",
    toast_resumed: "Guarding resumed",
    toast_restarted: "All guarded processes restarted",
    toast_history_cleared: "Crash history cleared",
    toast_config_reloaded: "Config updated, guarding reloaded",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_languages_are_complete() {
        let zh = strings(Lang::Zh);
        let en = strings(Lang::En);
        assert!(zh.title.contains("进程守护"));
        assert!(en.title.contains("Process Watch"));
        assert_ne!(zh.empty_title, en.empty_title);
        assert_ne!(zh.add_button, en.add_button);
        assert_ne!(zh.menu_file, en.menu_file);
        assert_ne!(zh.about_body, en.about_body);
    }

    #[test]
    fn default_matches_prototype() {
        // The approved prototype opens in dark theme with Chinese copy.
        assert_eq!(Settings::default().theme, Theme::Dark);
        assert_eq!(Settings::default().lang, Lang::Zh);
    }

    #[test]
    fn lang_and_theme_json_roundtrip() {
        assert_eq!(serde_json::to_string(&Lang::Zh).unwrap(), "\"zh\"");
        assert_eq!(serde_json::to_string(&Lang::En).unwrap(), "\"en\"");
        assert_eq!(serde_json::to_string(&Theme::Dark).unwrap(), "\"dark\"");
        let l: Lang = serde_json::from_str("\"en\"").unwrap();
        assert_eq!(l, Lang::En);
        let s: Settings = serde_json::from_str("{\"lang\":\"en\",\"theme\":\"dark\"}").unwrap();
        assert_eq!(s.lang, Lang::En);
        assert_eq!(s.theme, Theme::Dark);
    }
}
