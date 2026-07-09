#!/usr/bin/env bash
# Phase 1 验收：模拟 adapter 发送事件序列，观察悬浮窗变化
set -euo pipefail
API=http://127.0.0.1:7799/api/events

post() {
  curl -s -X POST "$API" -H 'Content-Type: application/json' -d "$1" > /dev/null
  echo "sent: $1"
}

post '{"tool_id":"codex","event_type":"tool_connected"}'
post '{"tool_id":"claude_code","event_type":"tool_connected"}'
sleep 2
post '{"tool_id":"codex","event_type":"task_started","task_id":"demo-1","workspace":"LifeAdminPet","message":"running xcodebuild"}'
post '{"tool_id":"codex","event_type":"task_started","task_id":"demo-2","workspace":"LinkKit","message":"lint and test"}'
sleep 3
post '{"tool_id":"codex","event_type":"task_waiting","task_id":"demo-1","message":"confirm command: rm -rf dist"}'
sleep 3
post '{"tool_id":"codex","event_type":"task_error","task_id":"demo-2","message":"xcodebuild failed: exit 65"}'
sleep 3
post '{"tool_id":"codex","event_type":"task_done","task_id":"demo-1","message":"done"}'
sleep 8
post '{"tool_id":"codex","event_type":"tool_disconnected"}'
echo "demo finished — Codex 板块应已消失，仅剩 Claude Code 绿灯"
