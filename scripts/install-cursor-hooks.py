#!/usr/bin/env python3
"""把 Agent Status Board 的 Cursor hooks 合并进 ~/.cursor/hooks.json。

注册步骤：beforeSubmitPrompt(开始)、afterFileEdit/beforeShellExecution/
afterShellExecution/beforeReadFile(运行心跳)、stop/sessionEnd(完成)。
幂等：已存在相同 command 的条目不重复添加。
安全：写入前备份；保留用户现有 hook（如 rtk 的 preToolUse）。
生效：Cursor 监听 hooks.json，保存即热重载。
"""
import json
import os
import shutil
import sys
import time

HOOK_SCRIPT = os.path.expanduser(
    "~/dev/agent-status-board/adapters/cursor/asb_cursor_hook.py"
)
COMMAND = f"python3 {HOOK_SCRIPT}"
STEPS = [
    "beforeSubmitPrompt",
    "afterFileEdit",
    "beforeShellExecution",
    "afterShellExecution",
    "beforeReadFile",
    "stop",
    "sessionEnd",
]
CONFIG = os.path.expanduser("~/.cursor/hooks.json")


def main():
    dry_run = "--dry-run" in sys.argv
    if not os.path.exists(HOOK_SCRIPT):
        sys.exit(f"hook 脚本不存在: {HOOK_SCRIPT}")
    config = {"version": 1, "hooks": {}}
    if os.path.exists(CONFIG):
        with open(CONFIG, "r", encoding="utf-8") as f:
            config = json.load(f)
    config.setdefault("version", 1)
    hooks = config.setdefault("hooks", {})
    added = []
    for step in STEPS:
        entries = hooks.setdefault(step, [])
        if any(e.get("command") == COMMAND for e in entries):
            continue
        entries.append({"command": COMMAND})
        added.append(step)
    if dry_run:
        print(f"[dry-run] 将添加: {added or '（全部已存在）'}")
        print(json.dumps(config, ensure_ascii=False, indent=2))
        return
    if os.path.exists(CONFIG):
        backup = f"{CONFIG}.bak-{time.strftime('%Y%m%d-%H%M%S')}"
        shutil.copy2(CONFIG, backup)
        print(f"已备份: {backup}")
    with open(CONFIG, "w", encoding="utf-8") as f:
        json.dump(config, f, ensure_ascii=False, indent=2)
        f.write("\n")
    print(f"已添加 Cursor hooks: {added or '（全部已存在，无改动）'}")
    print("Cursor 会热重载 hooks.json；对进行中的会话下次提问即生效。")


if __name__ == "__main__":
    main()
