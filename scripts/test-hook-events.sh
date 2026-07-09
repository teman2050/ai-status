#!/usr/bin/env bash
# Phase 2 验收：用样例 hook JSON 驱动 asb_hook.py，模拟一次完整 Claude Code 会话
set -euo pipefail
cd "$(dirname "$0")/.."
HOOK="python3 adapters/claude-code/asb_hook.py"

send() { echo "$1" | $HOOK; echo "sent: $(echo "$1" | head -c 80)"; }

send '{"hook_event_name":"SessionStart","session_id":"probe-1","cwd":"/tmp/DemoProj"}'
send '{"hook_event_name":"SessionStart","session_id":"probe-2","cwd":"/tmp/OtherProj"}'
sleep 2
send '{"hook_event_name":"UserPromptSubmit","session_id":"probe-1","cwd":"/tmp/DemoProj"}'
send '{"hook_event_name":"PostToolUse","session_id":"probe-1","cwd":"/tmp/DemoProj","tool_name":"Bash"}'
sleep 2
send '{"hook_event_name":"Notification","session_id":"probe-1","cwd":"/tmp/DemoProj","message":"Claude needs your permission to use Bash"}'
sleep 3
send '{"hook_event_name":"Stop","session_id":"probe-1","cwd":"/tmp/DemoProj"}'
sleep 6
send '{"hook_event_name":"SessionEnd","session_id":"probe-1","cwd":"/tmp/DemoProj"}'
echo "此刻 probe-2 仍在线，claude_code 板块应保留："
curl -s http://127.0.0.1:7799/api/tools
send '{"hook_event_name":"SessionEnd","session_id":"probe-2","cwd":"/tmp/OtherProj"}'
echo "全部 session 结束，claude_code 板块应消失："
curl -s http://127.0.0.1:7799/api/tools
