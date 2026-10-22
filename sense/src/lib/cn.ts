export type ClassValue = string | false | null | undefined;

/** 拼接 className，过滤掉假值。D4 的义项卡与折叠态会用到。 */
export function cn(...values: ClassValue[]): string {
  return values.filter(Boolean).join(" ");
}
