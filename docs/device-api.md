# Device API v1（硬件显示端协议）

ESP32 等外接硬件与 AI Status 桌面端之间的通信协议。由桌面端 App 内置提供
(`src-tauri/src/device.rs`),在设置里打开"局域网设备 API"后生效;默认关闭,
关闭时 App 只监听 loopback,不暴露任何局域网端口。

Adapter 上报用的 loopback API(`127.0.0.1:7799`)与本协议无关,始终仅本机可见。

- 传输:HTTP/1.1,JSON,UTF-8
- 默认端口:`7788`(config.json `device_api_port` 可改)
- 鉴权:可选 token(config.json `device_api_token`)。设置后请求必须带
  `?token=xxx` 或 `X-Token: xxx` 头,否则 401
- 硬件端访问电脑的**局域网 IP**;IP 由下面的发现机制自动获得,无需写死

## 设备发现(配对)

电脑的局域网 IP 由 DHCP 分配、会变,硬件端不写死 IP,通过 UDP 广播配对:

```
设备                                桌面端 App
  │── UDP 广播到端口 7789 ──────────→│  载荷(ASCII): AISP_DISCOVER v1
  │←─ 单播应答(JSON)───────────────│  {"app":"aistatusplus","v":1,"port":7788,"name":"PC-NAME"}
  │  从应答包的源地址得到电脑 IP
```

- 应答中 `app` 固定为 `aistatusplus`(协议家族标识,设备用它过滤无关应答),
  `port` 是 HTTP API 端口,`name` 是电脑主机名(预留给多电脑绑定)
- 发现请求不校验 token(只泄露 IP+端口,HTTP 数据仍受 token 保护)
- 硬件端建议策略:上电用上次配对保存的 IP 直连;连续 3 次传输失败自动重新广播;
  同时发全局广播(255.255.255.255)和子网定向广播

## GET /api/ping

连通性测试,永远不需要 token。

```json
{ "ok": true, "app": "aistatusplus", "v": 1 }
```

## GET /api/device/summary

设备显示所需的全部状态,已按小屏做过截断,硬件端不需要再做业务加工。

```json
{
  "ok": true,
  "v": 1,
  "rev": "1a2b3c4d",
  "ts": 1752201234,
  "net": "ok",
  "tools": [
    {
      "id": "claude_code",
      "name": "Claude Code",
      "st": "RUN",
      "quota": null,
      "use": { "h5": 6, "wk": 1 },
      "tasks": [
        { "st": "RUN", "txt": "fix login bug", "sub": "editing server.rs", "ws": "aistatus", "min": 12, "tok": 52000 }
      ]
    }
  ]
}
```

| 字段 | 说明 |
|---|---|
| `rev` | 数据签名(8 位 hex)。**内容不变则 rev 不变**;硬件端用它判断"无变化 → 不刷屏"。计算只含稳定字段,不含 `min` / `tok` / `ts` / `use` |
| `ts` | 服务器 Unix 秒 |
| `net` | 桌面端网络探测:`ok` / `flaky` / `down` |
| `tools[].st` | 工具级状态,取其任务中优先级最高者:`ERR > WAIT > PAUSE > RUN > DONE`(与菜单栏一致);无任务时 `IDLE` |
| `tools[].quota` | 配额耗尽时的恢复时间短文本(如 `9:30pm`),否则 `null` |
| `tools[].use` | 配额用量百分比(目前仅 Codex),`{h5, wk}` 整数;可能缺省 |
| `tasks[].st` | `RUN` / `WAIT` / `PAUSE` / `ERR` / `DONE` |
| `tasks[].txt` | 任务标题,≤ 40 字符(服务端已截断) |
| `tasks[].sub` | 任务当前动作摘要,≤ 40 字符,可能为空 |
| `tasks[].ws` | 工作区名,≤ 16 字符 |
| `tasks[].min` | 已运行分钟数 |
| `tasks[].tok` | token 消耗量,可能缺省 |

状态映射(内部 → 设备):`running→RUN`,`waiting→WAIT`,`paused→PAUSE`,`error→ERR`,`done_event→DONE`,未知→`WAIT`。

容量上限:最多 6 个工具,每工具最多 6 个任务(防止撑爆硬件内存)。

## 错误结构

所有错误都是 `{"ok": false, "error": "<code>"}`:

| HTTP | error | 含义 |
|---|---|---|
| 401 | `unauthorized` | token 缺失或不匹配 |
| 404 | `not_found` | 路径不存在 |
| 502 | `desktop_unreachable` | 仅历史外置 bridge 会返回;App 内置实现不会出现,但硬件端仍应处理 |

## 建议轮询节奏

- 每 2 秒 `GET /api/device/summary`;`rev` 不变则不重绘
- HTTP 超时:连接 ≤ 800ms,读取 ≤ 1200ms,不要阻塞按键
- 手动刷新(按键)可立即请求一次

## 平台注意

- **Windows**:需放行入站 TCP 7788 + UDP 7789,见 `scripts/add-device-firewall.ps1`
- **macOS**:首次监听时系统会弹"是否允许接受传入网络连接",点允许即可
- 协议如有破坏性调整会递增 `v`;硬件端(私有仓库 aistatusplus)只适配本规范
