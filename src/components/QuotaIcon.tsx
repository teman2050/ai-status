/**
 * Quota-exhausted icon: a minimal red clock (turns red via currentColor).
 * Just a ring + hour hand + minute hand — no bells, no legs.
 * Shared by the expanded and compact widget, and the tool-level quota row.
 */
export function QuotaIcon() {
  return (
    <svg
      className="quota-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5l3.5 2" />
    </svg>
  );
}
