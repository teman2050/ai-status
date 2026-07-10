export type Lang = "zh" | "en";
export type LangSetting = "auto" | "zh" | "en";

type Dict = Record<string, { zh: string; en: string }>;

const T: Dict = {
  idle: { zh: "空闲", en: "Idle" },
  online: { zh: "在线", en: "Online" },
  lost: { zh: "失联，等待恢复", en: "Lost, reconnecting" },
  no_tools: { zh: "还没检测到 AI 工具", en: "No AI tools detected yet" },
  onboard_hint: {
    zh: "打开 Claude Code / Codex / Cursor 就会自动出现；要看逐任务状态，按 README 装上 hook 适配器。",
    en: "Open Claude Code / Codex / Cursor and they'll show up here. For per-task status, install the hook adapters (see README).",
  },
  sec_status: { zh: "状态", en: "Status" },
  sec_floating: { zh: "悬浮窗", en: "Floating window" },
  sec_appearance: { zh: "外观", en: "Appearance" },
  sec_tools: { zh: "工具", en: "Tools" },
  sec_general: { zh: "通用", en: "General" },
  show_floating: { zh: "显示悬浮窗", en: "Show floating window" },
  compact_floating: { zh: "紧凑悬浮窗", en: "Compact floating window" },
  opacity: { zh: "透明度", en: "Opacity" },
  always_on_top: { zh: "窗口置顶", en: "Always on top" },
  theme: { zh: "主题", en: "Theme" },
  dark: { zh: "深色", en: "Dark" },
  light: { zh: "浅色", en: "Light" },
  language: { zh: "语言", en: "Language" },
  menubar_progress: { zh: "菜单栏显示进度（最多 4）", en: "Menu-bar progress (max 4)" },
  launch_at_login: { zh: "开机自启", en: "Launch at login" },
  network_probe: { zh: "网络探测", en: "Network probe" },
  local_note: { zh: "本地运行 · 不联网", en: "Local only · offline" },
  quit: { zh: "退出", en: "Quit" },
  net_down: { zh: "断网", en: "Offline" },
  net_flaky: { zh: "网络抖动", en: "Unstable" },
  q_5h: { zh: "5h配额", en: "5h quota" },
  q_weekly: { zh: "周配额", en: "Weekly quota" },
  q_5h_s: { zh: "5h", en: "5h" },
  q_weekly_s: { zh: "周", en: "wk" },
  q_generic: { zh: "配额", en: "Quota" },
  q_reached: { zh: "用完", en: "reached" },
  q_left: { zh: "还剩", en: "resets in" },
  q_remaining: { zh: "剩余", en: "remaining" },
  q_low_title: { zh: "配额即将用尽", en: "Quota running low" },
  q_throttled: { zh: "服务限流，重试中", en: "Rate limited, retrying" },
  working: { zh: "处理中", en: "Working" },
  waiting_input: { zh: "等待输入", en: "Waiting for input" },
  failed: { zh: "执行失败", en: "Failed" },
};

let currentLang: Lang = "zh";

export function resolveLang(setting: LangSetting): Lang {
  if (setting === "zh" || setting === "en") return setting;
  const nav = typeof navigator !== "undefined" ? navigator.language : "en";
  return nav.toLowerCase().startsWith("zh") ? "zh" : "en";
}

export function setLang(setting: LangSetting) {
  currentLang = resolveLang(setting);
}

export function t(key: keyof typeof T): string {
  return T[key]?.[currentLang] ?? String(key);
}
