import type { Ref, SVGProps } from "react";
import { ICON_PATHS, type IconName } from "./paths";

type IconProps = {
  name: IconName;
  /** Rendered px width/height. Prototype default is 15. */
  size?: number;
  /** Stroke width. Prototype default is 1.7 (varies 1.5-2.4 per context). */
  strokeWidth?: number;
  ref?: Ref<SVGSVGElement>;
} & Omit<SVGProps<SVGSVGElement>, "ref">;

/**
 * Single-path stroked icon matching the prototype's inline SVG rendering:
 * viewBox 0 0 24 24, fill none, stroke currentColor, round caps/joins.
 */
export function Icon({
  name,
  size = 15,
  strokeWidth = 1.7,
  ref,
  ...rest
}: IconProps) {
  return (
    <svg
      ref={ref}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...rest}
    >
      <path d={ICON_PATHS[name]} />
    </svg>
  );
}
