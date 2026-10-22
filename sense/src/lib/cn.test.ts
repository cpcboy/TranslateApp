import { describe, expect, it } from "vitest";
import { cn } from "./cn";

describe("cn", () => {
  it("拼接多个类名", () => {
    expect(cn("a", "b")).toBe("a b");
  });

  it("过滤 null / undefined / false", () => {
    expect(cn("a", null, undefined, false, "b")).toBe("a b");
  });

  it("全部为假值时返回空串", () => {
    expect(cn(null, undefined, false)).toBe("");
  });
});
