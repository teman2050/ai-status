import { describe, expect, it } from "vitest";
import { formatElapsed, formatTokens, pruneDoneTasks, toolBadge, DONE_TTL_MS } from "../status";
import type { TaskInfo } from "../types";

function task(id: string, status: TaskInfo["visible_status"]): TaskInfo {
  return {
    task_id: id,
    tool_id: "codex",
    workspace: "Demo",
    title: "Demo",
    summary: "",
    visible_status: status,
    elapsed_seconds: 0,
    updated_at: "",
  };
}

describe("formatElapsed", () => {
  it("pads minutes:seconds", () => {
    expect(formatElapsed(0)).toBe("00:00");
    expect(formatElapsed(192)).toBe("03:12");
  });
  it("shows hours past one hour", () => {
    expect(formatElapsed(3792)).toBe("1:03:12");
  });
});

describe("formatTokens", () => {
  it("returns empty string for empty value", () => {
    expect(formatTokens(null)).toBe("");
    expect(formatTokens(undefined)).toBe("");
  });
  it("abbreviates thousands/millions", () => {
    expect(formatTokens(999)).toBe("999");
    expect(formatTokens(12300)).toBe("12.3k");
    expect(formatTokens(1200000)).toBe("1.2m");
  });
});

describe("toolBadge", () => {
  it("aggregates by priority: error > waiting > paused > running", () => {
    expect(toolBadge([task("a", "running"), task("b", "error")], "ok").cls).toBe("error");
    expect(toolBadge([task("a", "paused"), task("b", "waiting")], "ok").cls).toBe("waiting");
    expect(toolBadge([task("a", "running"), task("b", "paused")], "ok").cls).toBe("paused");
    expect(toolBadge([task("a", "running")], "ok")).toEqual({ glyph: "▶", cls: "running" });
  });
  it("no tasks -> network: offline red, healthy green dot", () => {
    expect(toolBadge([], "down").cls).toBe("net-down");
    expect(toolBadge([], "flaky").cls).toBe("net-flaky");
    expect(toolBadge([], "ok")).toEqual({ glyph: "●", cls: "online" });
  });
});

describe("pruneDoneTasks", () => {
  it("keeps done_event within TTL, removes after timeout", () => {
    const seen = new Map<string, number>();
    const t0 = 1000;
    expect(pruneDoneTasks([task("a", "done_event")], seen, t0)).toHaveLength(1);
    expect(pruneDoneTasks([task("a", "done_event")], seen, t0 + DONE_TTL_MS - 1)).toHaveLength(1);
    expect(pruneDoneTasks([task("a", "done_event")], seen, t0 + DONE_TTL_MS + 1)).toHaveLength(0);
  });
  it("non-done tasks are unaffected", () => {
    const seen = new Map<string, number>();
    expect(pruneDoneTasks([task("b", "running")], seen, 0)).toHaveLength(1);
  });
  it("clears firstSeen for tasks that vanish", () => {
    const seen = new Map<string, number>();
    pruneDoneTasks([task("a", "done_event")], seen, 0);
    pruneDoneTasks([], seen, 100);
    expect(seen.size).toBe(0);
  });
});
