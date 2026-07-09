#!/usr/bin/env python3
"""把 Agent Status Board 的 Claude Code hooks 合并进 ~/.claude/settings.json。

用法（参考 PromLight 的 setup claude --global 模式）：
    python3 scripts/install-claude-hooks.py            # 安装
    python3 scripts/install-claude-hooks.py --dry-run  # 只预览将写入的内容

幂等：已存在相同 command 的条目不重复添加。
安全：写入前备份为 settings.json.bak-<时间戳>；不改动其他配置（如 rtk hook）。
生效：hooks 只对新启动的 Claude Code 会话生效。
"""
import json
import os
import shutil
import sys
import time

HOOK_SCRIPT = os.path.expanduser(
    "~/dev/agent-status-board/adapters/claude-code/asb_hook.py"
)
COMMAND = f"python3 {HOOK_SCRIPT}"
EVENTS = [
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "PostToolUseFailure",
    "Notification",
    "Stop",
    "SessionEnd",
]
SETTINGS = os.path.expanduser("~/.claude/settings.json")


def main():
    dry_run = "--dry-run" in sys.argv
    if not os.path.exists(HOOK_SCRIPT):
        sys.exit(f"hook 脚本不存在: {HOOK_SCRIPT}")
    settings = {}
    if os.path.exists(SETTINGS):
        with open(SETTINGS, "r", encoding="utf-8") as f:
            settings = json.load(f)
    hooks = settings.setdefault("hooks", {})
    added = []
    for event in EVENTS:
        entries = hooks.setdefault(event, [])
        already = any(
            h.get("command") == COMMAND
            for entry in entries
            for h in entry.get("hooks", [])
        )
        if already:
            continue
        entries.append(
            {
                "hooks": [
                    {
                        "type": "command",
                        "command": COMMAND,
                        "timeout": 5,
                        "async": True,
                    }
                ]
            }
        )
        added.append(event)
    if dry_run:
        print(f"[dry-run] 将添加 hooks: {added or '（全部已存在，无改动）'}")
        print(json.dumps({"hooks": hooks}, ensure_ascii=False, indent=2))
        return
    if os.path.exists(SETTINGS):
        backup = f"{SETTINGS}.bak-{time.strftime('%Y%m%d-%H%M%S')}"
        shutil.copy2(SETTINGS, backup)
        print(f"已备份: {backup}")
    with open(SETTINGS, "w", encoding="utf-8") as f:
        json.dump(settings, f, ensure_ascii=False, indent=2)
        f.write("\n")
    print(f"已添加 hooks: {added or '（全部已存在，无改动）'}")
    print("注意：hooks 对新启动的 Claude Code 会话生效，已开的会话不受影响。")


if __name__ == "__main__":
    main()
