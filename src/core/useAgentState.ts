import { useEffect, useRef, useState } from "react";
import { fetchSnapshot } from "./datasource";
import { mockSnapshot } from "./mock";
import { pruneDoneTasks } from "./status";
import type { Snapshot } from "./types";
import { EMPTY_SNAPSHOT } from "./types";

// Poll fast enough that idle->running (and other status flips) show promptly: the hook writes
// the event to the store in ~50ms, so the poll interval is the dominant client-side latency.
// The floating widget is lightweight and the local server answers in ~25ms, so 350ms is cheap.
const POLL_MS = 350;

export function useAgentState(useMock: boolean): Snapshot {
  const [snap, setSnap] = useState<Snapshot>(EMPTY_SNAPSHOT);
  const doneSeen = useRef(new Map<string, number>());

  useEffect(() => {
    let alive = true;
    async function tick() {
      const raw = useMock ? mockSnapshot() : await fetchSnapshot();
      if (!alive) return;
      // dev helper: add ?net=down / ?net=flaky to the URL to force the network state for visual checks
      const forced = new URLSearchParams(window.location.search).get("net");
      const network =
        forced === "down" || forced === "flaky" || forced === "ok"
          ? forced
          : raw.network;
      setSnap({
        tools: raw.tools,
        tasks: pruneDoneTasks(raw.tasks, doneSeen.current, Date.now()),
        network,
      });
    }
    tick();
    const id = setInterval(tick, POLL_MS);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [useMock]);

  return snap;
}
