import type { Snapshot } from "./types";
import { EMPTY_SNAPSHOT } from "./types";

const BASE = "http://127.0.0.1:7799";

export async function fetchSnapshot(): Promise<Snapshot> {
  try {
    const [toolsRes, tasksRes] = await Promise.all([
      fetch(`${BASE}/api/tools`),
      fetch(`${BASE}/api/tasks`),
    ]);
    const toolsBody = await toolsRes.json();
    const tasks = (await tasksRes.json()).tasks ?? [];
    return {
      tools: toolsBody.tools ?? [],
      tasks,
      network: toolsBody.network ?? "ok",
    };
  } catch {
    return EMPTY_SNAPSHOT; // show an empty board when the service isn't ready
  }
}
