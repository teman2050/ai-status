#!/usr/bin/env python3
"""把状态板接入 codex notify，保留用户原有 notifier（链式调用）。

原理：codex 只允许一个 notify 程序。本脚本读取 config.toml 中现有 notify 数组，
生成链式脚本（先调原 notifier，再调 asb_notify.py），把 notify 指向链式脚本。
无原 notify 时直接指向 asb_notify.py。幂等：已指向本项目脚本则不改。
"""
import os
import re
import shutil
import sys
import time

CONFIG = os.path.expanduser("~/.codex/config.toml")
ADAPTER = os.path.expanduser("~/dev/agent-status-board/adapters/codex/asb_notify.py")
CHAIN = os.path.expanduser("~/dev/agent-status-board/adapters/codex/asb_notify_chain.sh")


def main():
    if not os.path.exists(ADAPTER):
        sys.exit(f"适配器不存在: {ADAPTER}")
    text = ""
    if os.path.exists(CONFIG):
        with open(CONFIG, "r", encoding="utf-8") as f:
            text = f.read()
    match = re.search(r'^notify\s*=\s*\[(.*)\]\s*$', text, re.MULTILINE)
    if match and ("asb_notify" in match.group(1)):
        print("已接入，无改动。")
        return
    if match:
        original = re.findall(r'"((?:[^"\\]|\\.)*)"', match.group(1))
        quoted = " ".join(f'"{p}"' for p in original)
        with open(CHAIN, "w", encoding="utf-8") as f:
            f.write(
                "#!/usr/bin/env bash\n"
                "# Agent Status Board 链式 notify：先调原 notifier，再上报状态板\n"
                f'{quoted} "$@" 2>/dev/null || true\n'
                f'python3 "{ADAPTER}" "$@" 2>/dev/null || true\n'
                "exit 0\n"
            )
        os.chmod(CHAIN, 0o755)
        new_line = f'notify = ["bash", "{CHAIN}"]'
        new_text = text[: match.start()] + new_line + text[match.end():]
        print(f"原 notifier 已保留在链中: {original}")
    else:
        new_line = f'notify = ["python3", "{ADAPTER}"]'
        new_text = (text.rstrip() + "\n\n" if text.strip() else "") + new_line + "\n"
    if os.path.exists(CONFIG):
        backup = f"{CONFIG}.bak-{time.strftime('%Y%m%d-%H%M%S')}"
        shutil.copy2(CONFIG, backup)
        print(f"已备份: {backup}")
    with open(CONFIG, "w", encoding="utf-8") as f:
        f.write(new_text)
    print(f"已写入: {new_line}")
    print("注意：对新启动的 codex 会话生效。")


if __name__ == "__main__":
    main()
