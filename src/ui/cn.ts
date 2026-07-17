/** Minimal class-name joiner (no clsx dependency). Drops falsy values. */
export function cn(
  ...parts: Array<string | false | null | undefined>
): string {
  return parts.filter(Boolean).join(" ");
}
