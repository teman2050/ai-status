# AI STATUS 协作约定（Mac 为主，Windows 适配）

本仓库由两方维护：**Mac 侧**（功能主导，Claude 在用户 Mac 上开发）和 **Windows 侧**（平台适配，维护者在 Windows 机器上开发打包）。任何一方（包括各自使用的 AI 工具）动手前先读这份约定。

## 分工原则

- **新功能一律先在共用层实现**（Mac 侧负责），Windows 侧只做平台适配，不在共用层重复实现同一需求。
- 抓取逻辑（transcript / rollout 日志解析、HTTP 事件、状态机）是**平台无关的共用代码**：数据格式两端一致，只有路径前缀不同。修一处，两端受益；不要按平台复制。
- 展示逻辑（status.ts、组件、共用样式）同样共用。平台差异只落在下面的"平台专属"文件里。

## 文件所有权

| 区域 | 负责方 |
|---|---|
| `src/`（除 styles.win.css）、`src-tauri/src/*.rs` 共用逻辑、`.github/workflows/` | Mac 侧 |
| `src/styles.win.css`（全部 `.board.win` / `.panel.win` 覆写） | Windows 侧 |
| `scripts/*.ps1`、Windows 安装/验证脚本 | Windows 侧 |
| `src-tauri` 里 `#[cfg(windows)]` 分支（watcher 进程枚举、lib.rs 窗口行为） | Windows 侧 |

想动对方地盘：可以，但先在 release notes / commit message 里说清楚，并保证对方平台能编译。

**计划中**：把 `watcher.rs` / `lib.rs` 里的 `#[cfg(windows)]` 代码抽成独立 `win_*.rs` 模块，由 Windows 侧在能编译验证的机器上执行（Mac 侧无法验证 windows-sys 代码，不做这个搬移）。

## 版本与发布

- 版本号 `a.b.c`：`a.b` 由用户指定；`c`（patch）谁发版谁 +1。
- **打 tag 前必须** `git fetch` 并用 `git ls-remote --tags origin` 确认远端最大版本号，在其上 +1——两侧都发过版，本地认知会过期。
- 版本号同步 4 处：`package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`Cargo.lock`（跑一次 cargo 即更新）。
- CI（tag `v*` 触发）只构建 **Mac universal dmg** 并建 draft release；Windows exe/msi 由 Windows 侧手动构建后传到**同一个** release；draft 由用户点 Publish。

## 推送纪律

- 开工前 `git fetch` 看远端；改完**尽快** commit + push，不要攒几个小时再推。
- 推送被拒 = 对方刚推过：老实 merge/rebase，共用展示层以**先发布者为准**，自己未发布的重复实现让位，只叠加对方没有的修复。
- 提交信息说明改动属于共用层还是平台层，方便对方决定要不要跟进适配。

## 验收

- Mac 侧：`cargo test`（src-tauri）+ `npx vitest run` + `npx tsc --noEmit` + `npm run build` 全绿再推。
- 不跑真实 AI 会话烧 API 验收；用样例 JSON / 模拟数据验证。
