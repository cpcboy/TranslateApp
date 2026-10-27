import { describe, expect, it } from "vitest";
import { isUnsavedForm } from "./form";

const saved = { base_url: "https://api.deepseek.com/v1", model: "deepseek-chat" };

describe("测试连接：表单和已保存的配置有没有区别", () => {
  it("一样就不注明（尾部斜杠、空白不算区别）", () => {
    expect(isUnsavedForm(saved, { baseUrl: "https://api.deepseek.com/v1/", model: " deepseek-chat ", key: "" })).toBe(false);
  });

  it("地址、模型、新填的 Key 任何一个不同都要注明", () => {
    expect(isUnsavedForm(saved, { baseUrl: "https://gateway.example.com/v1", model: "deepseek-chat", key: "" })).toBe(true);
    expect(isUnsavedForm(saved, { baseUrl: saved.base_url, model: "other", key: "" })).toBe(true);
    expect(isUnsavedForm(saved, { baseUrl: saved.base_url, model: saved.model, key: "sk-new" })).toBe(true);
  });
});
