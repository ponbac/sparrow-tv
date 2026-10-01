import "react";

declare module "react" {
  /** Lets inline styles hand a number or length to a stylesheet rule. */
  interface CSSProperties {
    [name: `--${string}`]: string | number | undefined;
  }
}
