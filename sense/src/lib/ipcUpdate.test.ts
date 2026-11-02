import { beforeEach, describe, expect, it, vi } from "vitest";
// 用 Vite 的 ?raw 读 Rust / 前端源码：前后端的接线（命令名、参数名、事件名、字段、文案里的数字）只能靠对拍，不引入 Node 类型依赖
import appTsx from "../App.tsx?raw";
import describeTs from "../features/update/describe.ts?raw";
import settingsViewTsx from "../features/settings/SettingsView.tsx?raw";
import libRs from "../../src-tauri/src/lib.rs?raw";
import updaterRs from "../../src-tauri/src/updater.rs?raw";
import fixture from "./updateStatus.fixture.json";
import { phaseSchema, updateStatusSchema } from "./updateStatus";

const tauri = vi.hoisted(() => ({
  invoke: [] as Array<[string, unknown]>,
  result: null as unknown,
  listen: [] as Array<{ name: string; callback: (event: { payload: unknown }) => void }>,
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: unknown) => {
    tauri.invoke.push([command, args]);
    return Promise.resolve(tauri.result);
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, callback: (event: { payload: unknown }) => void) => {
    tauri.listen.push({ name, callback });
    return Promise.resolve(() => {});
  },
}));

import { api, onUpdateStatus } from "./ipc";

const samples = fixture as Record<string, Record<string, unknown>>;

beforeEach(() => {
  tauri.invoke = [];
  tauri.listen = [];
  tauri.result = samples.up_to_date;
  vi.restoreAllMocks();
});

const camel = (name: string) => name.replace(/_([a-z])/g, (_, letter: string) => letter.toUpperCase());
const snake = (name: string) => name.replace(/([a-z])([A-Z])/g, "$1_$2").toLowerCase();

/** `pub (async )?fn NAME(window: WebviewWindow, app: AppHandle, enabled: bool)` → ["enabled"]（窗口和 AppHandle 是 Tauri 自己注入的，不是前端传的） */
function rustArgs(name: string): string[] {
  const match = new RegExp(`pub (?:async )?fn ${name}\\(([^)]*)\\)`).exec(updaterRs);
  if (!match) throw new Error(`updater.rs 里找不到命令 ${name}`);
  return match[1]
    .split(",")
    .map((param) => param.trim().split(":")[0].trim())
    .filter((arg) => arg && arg !== "window" && arg !== "app");
}

function rustStructFields(struct: string): string[] {
  const match = new RegExp(`pub struct ${struct} \\{([\\s\\S]*?)\\n\\}`).exec(updaterRs);
  if (!match) throw new Error(`updater.rs 里找不到 struct ${struct}`);
  return [...match[1].matchAll(/^\s*pub (\w+):/gm)].map((field) => field[1]);
}

function rustEnumVariants(name: string): string[] {
  const match = new RegExp(`pub enum ${name} \\{([\\s\\S]*?)\\n\\}`).exec(updaterRs);
  if (!match) throw new Error(`updater.rs 里找不到 enum ${name}`);
  return [...match[1].matchAll(/^ {4}([A-Z][A-Za-z]*)\b/gm)].map((variant) => variant[1]);
}

describe("前端命令 ↔ Rust 命令", () => {
  it("四个更新命令：名字登记进了 generate_handler!，传的参数名（驼峰）与 Rust 签名一一对上", async () => {
    await api.updateStatus();
    await api.checkUpdate();
    await api.restartForUpdate();
    await api.setAutoUpdate(true);
    expect(tauri.invoke.map(([command]) => command)).toEqual(["update_status", "update_check_now", "update_restart", "set_auto_update"]);
    for (const [command, args] of tauri.invoke) {
      expect(libRs, `${command} 没登记进 generate_handler!`).toContain(`updater::${command},`);
      expect(Object.keys((args as object | undefined) ?? {}).sort(), command).toEqual(rustArgs(command).map(camel).sort());
    }
  });

  it("开关把值原样传下去（true / false 都对），键名是 Rust 要的 enabled", async () => {
    await api.setAutoUpdate(true);
    await api.setAutoUpdate(false);
    expect(tauri.invoke).toEqual([
      ["set_auto_update", { enabled: true }],
      ["set_auto_update", { enabled: false }],
    ]);
  });

  it("命令返回的状态格式不认识：是看得见的错误（前后端版本对不上），不是悄悄当成功", async () => {
    tauri.result = { phase: { state: "teleporting" } };
    await expect(api.updateStatus()).rejects.toThrow("更新状态的格式不认识");
    await expect(api.checkUpdate()).rejects.toThrow("更新状态的格式不认识");
    await expect(api.setAutoUpdate(true)).rejects.toThrow("更新状态的格式不认识");
    tauri.result = null;
    await expect(api.updateStatus()).rejects.toThrow("更新状态的格式不认识");
  });

  it("命令返回合法状态就原样解析出来", async () => {
    tauri.result = samples.ready;
    const status = await api.checkUpdate();
    expect(status.phase).toEqual({ state: "ready", version: "0.1.1" });
  });
});

describe("状态事件", () => {
  it("监听的事件名就是 Rust 发的那个（EVENT 常量）", async () => {
    await onUpdateStatus(() => {});
    const match = /pub const EVENT: &str = "([^"]+)";/.exec(updaterRs);
    expect(match?.[1]).toBeTruthy();
    expect(tauri.listen.map((item) => item.name)).toEqual([match![1]]);
  });

  it("载荷合法交给 handler；不合法不交给 handler，但打一行错误并通知 onInvalid（别让整块更新界面悄无声息地消失）", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const handler = vi.fn();
    const onInvalid = vi.fn();
    await onUpdateStatus(handler, onInvalid);
    const { callback } = tauri.listen[0];

    callback({ payload: samples.ready });
    expect(handler).toHaveBeenCalledTimes(1);
    expect(handler.mock.calls[0][0].phase).toEqual({ state: "ready", version: "0.1.1" });
    expect(onInvalid).not.toHaveBeenCalled();
    expect(error).not.toHaveBeenCalled();

    callback({ payload: { seq: 1 } });
    callback({ payload: "oops" });
    expect(handler).toHaveBeenCalledTimes(1);
    expect(onInvalid).toHaveBeenCalledTimes(2);
    expect(error).toHaveBeenCalledTimes(2);
  });

  it("没传 onInvalid 也不抛", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    await onUpdateStatus(vi.fn());
    expect(() => tauri.listen[0].callback({ payload: 42 })).not.toThrow();
  });
});

describe("状态的字段 ↔ Rust", () => {
  it("UpdateStatus 的字段：Rust 的 struct、zod 的 schema、夹具三方一致（加字段忘了哪一边，哪一边红）", () => {
    const rust = rustStructFields("UpdateStatus").sort();
    expect(Object.keys(updateStatusSchema.shape).sort()).toEqual(rust);
    for (const [name, sample] of Object.entries(samples)) {
      expect(Object.keys(sample).sort(), name).toEqual(rust);
    }
  });

  it("最近一次更新（LastUpdate）的字段同样一致", () => {
    const rust = rustStructFields("LastUpdate").sort();
    expect(Object.keys(updateStatusSchema.shape.last_update.unwrap().shape).sort()).toEqual(rust);
  });

  it("Phase 的每个变体：Rust 里有的 zod 认得、夹具里有样本；zod 认得的 Rust 里都有", () => {
    const rust = rustEnumVariants("Phase").map(snake).sort();
    const zod = phaseSchema.options.map((option) => option.shape.state.value as string).sort();
    expect(zod).toEqual(rust);
    const inFixture = new Set(Object.values(samples).map((sample) => (sample.phase as { state: string }).state));
    for (const state of rust) expect(inFixture.has(state), `夹具里没有 ${state} 阶段的样本`).toBe(true);
  });

  it("ErrorKind 的取值两边一致，夹具里两种都有样本", () => {
    const rust = rustEnumVariants("ErrorKind").map(snake).sort();
    expect([...updateStatusSchema.shape.error_kind.unwrap().options].sort()).toEqual(rust);
    const kinds = new Set(Object.values(samples).map((sample) => sample.error_kind));
    for (const kind of rust) expect(kinds.has(kind), `夹具里没有 error_kind=${kind} 的样本`).toBe(true);
  });
});

describe("界面接线", () => {
  // 整行只有这个元素才算挂上了：`{false && <UpdateBanner … />}`、被注释掉的、写在别的行里的，子串匹配都会放过
  const onItsOwnLine = (element: string) => new RegExp(`^[ \\t]*${element}[ \\t]*$`, "m");

  it("App 里建一份更新状态，顶部横幅和设置页共用它", () => {
    expect(appTsx).toMatch(/^\s*const update = useUpdate\(\);\s*$/m);
    expect(appTsx).toMatch(onItsOwnLine("<UpdateBanner update=\\{update\\} />"));
    expect(appTsx).toMatch(onItsOwnLine('<SettingsView onBack=\\{\\(\\) => setView\\("lookup"\\)\\} update=\\{update\\} />'));
  });

  it("设置页把同一份更新状态交给「更新」卡片", () => {
    expect(settingsViewTsx).toMatch(onItsOwnLine("<UpdateCard update=\\{update\\} />"));
  });

  it("这个检测本身是灵的：认得出正常写法，认不出被关掉 / 注释掉 / 挤在别的东西后面的", () => {
    const re = onItsOwnLine("<UpdateBanner update=\\{update\\} />");
    expect(re.test("  <UpdateBanner update={update} />")).toBe(true);
    expect(re.test("  {false && <UpdateBanner update={update} />}")).toBe(false);
    expect(re.test("  {/* <UpdateBanner update={update} /> */}")).toBe(false);
    expect(re.test("  // <UpdateBanner update={update} />")).toBe(false);
    expect(re.test("<div><UpdateBanner update={update} /></div>")).toBe(false);
  });
});

describe("文案里的数字 ↔ Rust 的常量", () => {
  it("「启动半分钟后会自动检查，之后每 6 小时一次」：首次检查延迟 30 秒、检查间隔 6 小时", () => {
    const delay = /const FIRST_CHECK_DELAY: Duration = Duration::from_secs\((\d+)\);/.exec(updaterRs);
    const interval = /const CHECK_INTERVAL_MS: u64 = (\d+) \* 60 \* 60 \* 1000;/.exec(updaterRs);
    expect(Number(delay?.[1])).toBe(30);
    expect(Number(interval?.[1])).toBe(6);
    expect(describeTs).toContain("启动半分钟后会自动检查，之后每 6 小时一次");
  });
});
