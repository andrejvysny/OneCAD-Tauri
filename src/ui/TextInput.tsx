import type { InputHTMLAttributes, Ref } from "react";
import { cn } from "./cn";
import { Icon } from "@/icons/Icon";
import type { IconName } from "@/icons/paths";

type TextInputProps = {
  /** Optional leading glyph (prototype search field uses `search`). */
  leadingIcon?: IconName;
  className?: string;
  wrapperClassName?: string;
  ref?: Ref<HTMLInputElement>;
} & InputHTMLAttributes<HTMLInputElement>;

/** 32px text field with hairline border + accent focus ring. */
export function TextInput({
  leadingIcon,
  className,
  wrapperClassName,
  ref,
  ...rest
}: TextInputProps) {
  return (
    <div
      className={cn("relative inline-flex items-center", wrapperClassName)}
    >
      {leadingIcon && (
        <Icon
          name={leadingIcon}
          size={14}
          strokeWidth={1.8}
          className="pointer-events-none absolute left-[9px] text-ink-6"
        />
      )}
      <input
        ref={ref}
        className={cn(
          "h-[32px] w-full rounded-sm border border-border-strong bg-white",
          "font-ui text-[13px] text-ink outline-none placeholder:text-ink-6",
          "focus:border-accent focus:shadow-focus-ring",
          leadingIcon ? "pl-[30px] pr-2.5" : "px-2.5",
          className,
        )}
        {...rest}
      />
    </div>
  );
}
