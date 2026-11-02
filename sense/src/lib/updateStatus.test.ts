import { describe, expect, it } from "vitest";
import fixture from "./updateStatus.fixture.json";
import { parseUpdateStatus } from "./updateStatus";

const samples = fixture as Record<string, unknown>;

describe("更新状态的解析（与 Rust 共用同一份夹具）", () => {
  // 夹具的键 → 期望的 phase.state。Rust 侧断言序列化结果等于这份夹具，所以这里解得出来 = 两边对得上
  const expectedState: Record<string, string> = {
    idle: "idle",
    checking: "checking",
    up_to_date: "up_to_date",
    downloading: "downloading",
    downloading_unknown_size: "downloading",
    ready: "ready",
    installing: "installing",
    failed_check: "idle",
    unavailable: "idle",
    failed_install: "idle",
    ready_install_failed: "ready",
  };

  it("夹具里的每一种状态都能解析，阶段对得上", () => {
    expect(Object.keys(samples).sort()).toEqual(Object.keys(expectedState).sort());
    for (const [name, raw] of Object.entries(samples)) {
      const status = parseUpdateStatus(raw);
      expect(status, name).not.toBeNull();
      expect(status?.phase.state, name).toBe(expectedState[name]);
    }
  });

  it("带版本的阶段把版本和进度带过来；服务器没给长度时 total 是 null，不是 0", () => {
    const downloading = parseUpdateStatus(samples.downloading);
    expect(downloading?.phase).toEqual({
      state: "downloading",
      version: "0.1.1",
      downloaded: 2097152,
      total: 9437184,
    });
    const unknown = parseUpdateStatus(samples.downloading_unknown_size);
    expect(unknown?.phase).toMatchObject({ state: "downloading", total: null });
  });

  it("失败原因、失败属于哪一类、不可用原因、最近一次更新都原样带过来", () => {
    expect(parseUpdateStatus(samples.failed_check)?.last_error).toContain("GitHub");
    expect(parseUpdateStatus(samples.failed_check)?.error_kind).toBe("check");
    expect(parseUpdateStatus(samples.failed_install)?.error_kind).toBe("install");
    expect(parseUpdateStatus(samples.ready_install_failed)).toMatchObject({
      phase: { state: "ready", version: "0.1.1" },
      error_kind: "install",
    });
    expect(parseUpdateStatus(samples.ready)?.error_kind).toBeNull();
    const unavailable = parseUpdateStatus(samples.unavailable);
    expect(unavailable?.unavailable).toContain("开发版");
    expect(unavailable?.auto_update).toBe(false);
    expect(parseUpdateStatus(samples.ready)?.last_update).toEqual({
      from: "0.0.9",
      to: "0.1.0",
      at: 1767139200000,
    });
  });

  it("认不出来的载荷一律返回 null，不抛：未知阶段、缺字段、字段类型不对、不是对象", () => {
    const good = samples.ready as Record<string, unknown>;
    expect(parseUpdateStatus({ ...good, phase: { state: "teleporting" } })).toBeNull();
    expect(parseUpdateStatus({ ...good, phase: { state: "ready" } })).toBeNull(); // ready 必须带版本
    const { seq: _seq, ...withoutSeq } = good;
    expect(parseUpdateStatus(withoutSeq)).toBeNull();
    expect(parseUpdateStatus({ ...good, auto_update: "yes" })).toBeNull();
    expect(parseUpdateStatus({ ...good, error_kind: "network" })).toBeNull(); // 只认 check / install
    const { error_kind: _kind, ...withoutKind } = good;
    expect(parseUpdateStatus(withoutKind)).toBeNull();
    for (const junk of [null, undefined, 42, "ready", [], {}]) {
      expect(parseUpdateStatus(junk)).toBeNull();
    }
  });

  it("夹具遵守 Rust 的约定：有失败原因就有类型，没有失败原因就没有类型", () => {
    for (const [name, raw] of Object.entries(samples)) {
      const status = parseUpdateStatus(raw);
      expect(status?.last_error === null, name).toBe(status?.error_kind === null);
    }
  });

  it("多出来的未知字段不让解析失败（以后只加可选字段）", () => {
    expect(parseUpdateStatus({ ...(samples.ready as object), future_field: 1 })).not.toBeNull();
  });
});
