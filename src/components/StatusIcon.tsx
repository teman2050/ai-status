import { GLYPHS } from "../core/status";
import type { VisibleStatus } from "../core/types";

export function StatusIcon({ status }: { status: VisibleStatus }) {
  return <span className={`status-icon ${status}`}>{GLYPHS[status]}</span>;
}
