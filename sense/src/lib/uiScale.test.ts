import { describe, expect, it } from "vitest";
import {
  clampScale,
  describeScale,
  loadScale,
  nextScale,
  SCALE_DEFAULT,
  SCALE_MAX,
  SCALE_MIN,
  saveScale,
  scaleActionFor,
  STORAGE_KEY,
} from "./uiScale";

type Key = Parameters<typeof scaleActionFor>[0];
/** 一次 ⌘ 加某个键的按键；其余字段给定常的默认值 */
const press = (key: string, extra: Partial<Key> = {}): Key => ({
  key,
  code: "",
  metaKey: true,
  ctrlKey: false,
  altKey: false,
  isComposing: false,
  keyCode: 0,
  ...extra,
});

describe("缩放档位", () => {
  it("每次 10 个百分点，来回不走样（浮点误差被收掉）", () => {
    let value = SCALE_DEFAULT;
    const up = [];
    for (let i = 0; i < 6; i += 1) {
      value = nextScale(value, "in");
      up.push(value);
    }
    expect(up).toEqual([1.1, 1.2, 1.3, 1.4, 1.5, 1.6]);
    for (let i = 0; i < 6; i += 1) value = nextScale(value, "out");
    expect(value).toBe(1);
  });

  it("到头就停在头上：不会超过最大 / 最小", () => {
    expect(nextScale(SCALE_MAX, "in")).toBe(SCALE_MAX);
    expect(nextScale(SCALE_MIN, "out")).toBe(SCALE_MIN);
    let value = 1;
    for (let i = 0; i < 50; i += 1) value = nextScale(value, "in");
    expect(value).toBe(SCALE_MAX);
    for (let i = 0; i < 50; i += 1) value = nextScale(value, "out");
    expect(value).toBe(SCALE_MIN);
  });

  it("⌘0：不管现在多大，一步回到 100%", () => {
    expect(nextScale(1.6, "reset")).toBe(1);
    expect(nextScale(0.7, "reset")).toBe(1);
  });

  it("clampScale：越界夹住，不是数就回默认", () => {
    expect(clampScale(9)).toBe(SCALE_MAX);
    expect(clampScale(0.01)).toBe(SCALE_MIN);
    expect(clampScale(Number.NaN)).toBe(SCALE_DEFAULT);
    expect(clampScale(Number.POSITIVE_INFINITY)).toBe(SCALE_DEFAULT);
    expect(clampScale(1.23456)).toBe(1.23);
  });
});

describe("哪些按键算缩放快捷键", () => {
  it("⌘+：⌘= 和 ⌘⇧= 都放大（美式键盘上 + 要按 Shift）", () => {
    expect(scaleActionFor(press("="))).toBe("in");
    expect(scaleActionFor(press("+", { shiftKey: true } as Partial<Key>))).toBe("in");
    expect(scaleActionFor(press("+"))).toBe("in");
  });

  it("⌘-：缩小（⌘⇧- 打出来是 _，也算）", () => {
    expect(scaleActionFor(press("-"))).toBe("out");
    expect(scaleActionFor(press("_"))).toBe("out");
  });

  it("⌘0：还原", () => {
    expect(scaleActionFor(press("0"))).toBe("reset");
  });

  it("小键盘的 + - 0 也行（WebKit 里 key 就是 + - 0，code 是 NumpadAdd 等）", () => {
    expect(scaleActionFor(press("+", { code: "NumpadAdd" }))).toBe("in");
    expect(scaleActionFor(press("-", { code: "NumpadSubtract" }))).toBe("out");
    expect(scaleActionFor(press("0", { code: "Numpad0" }))).toBe("reset");
  });

  it("小键盘：key 取不到（Unidentified）时靠 code 兜底", () => {
    expect(scaleActionFor(press("Unidentified", { code: "NumpadAdd" }))).toBe("in");
    expect(scaleActionFor(press("Unidentified", { code: "NumpadSubtract" }))).toBe("out");
    expect(scaleActionFor(press("Unidentified", { code: "Numpad0" }))).toBe("reset");
  });

  it("按字符认，不按物理位置：别的布局上 = 位置的键不会被误当成放大", () => {
    // 德语布局里美式 = 的位置是 ´（死键）：key 不是 = 也不是 +，不能响应
    expect(scaleActionFor(press("´", { code: "Equal" }))).toBeNull();
    expect(scaleActionFor(press("ß", { code: "Minus" }))).toBeNull();
  });

  it("没按 ⌘ 不响应：输入框里正常打 + - 0 不受影响", () => {
    for (const key of ["=", "+", "-", "0"]) {
      expect(scaleActionFor(press(key, { metaKey: false })), key).toBeNull();
    }
  });

  it("带 ⌃ 或 ⌥ 的不响应：留给别的快捷键", () => {
    expect(scaleActionFor(press("=", { ctrlKey: true }))).toBeNull();
    expect(scaleActionFor(press("-", { altKey: true }))).toBeNull();
    expect(scaleActionFor(press("0", { ctrlKey: true, altKey: true }))).toBeNull();
  });

  it("输入法组字中不拦（isComposing，或 WebKit 组字收尾那下的 keyCode 229）", () => {
    expect(scaleActionFor(press("=", { isComposing: true }))).toBeNull();
    expect(scaleActionFor(press("-", { keyCode: 229 }))).toBeNull();
  });

  it("别的 ⌘ 组合不碰：⌘C ⌘V ⌘Q ⌘W ⌘数字", () => {
    for (const key of ["c", "v", "q", "w", "1", "9", "a"]) {
      expect(scaleActionFor(press(key)), key).toBeNull();
    }
  });
});

describe("屏幕角落的提示", () => {
  it("正常调整：只报当前字号", () => {
    expect(describeScale("in", 1, 1.1)).toBe("字号 110%");
    expect(describeScale("out", 1, 0.9)).toBe("字号 90%");
  });

  it("到头了要说：按了没变化不能像坏了一样沉默", () => {
    expect(describeScale("in", SCALE_MAX, SCALE_MAX)).toBe("字号 160%（已是最大）");
    expect(describeScale("out", SCALE_MIN, SCALE_MIN)).toBe("字号 70%（已是最小）");
  });

  it("还原", () => {
    expect(describeScale("reset", 1.4, 1)).toBe("字号 100%（已还原）");
  });
});

describe("本地偏好的读写", () => {
  const storageWith = (value: string | null) => ({ getItem: (key: string) => (key === STORAGE_KEY ? value : null) });

  it("存什么读什么", () => {
    const data = new Map<string, string>();
    const storage = {
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => void data.set(key, value),
    };
    saveScale(1.3, storage);
    expect(data.get(STORAGE_KEY)).toBe("1.3");
    expect(loadScale(storage)).toBe(1.3);
  });

  it("没存过、存坏了：回默认", () => {
    expect(loadScale(storageWith(null))).toBe(SCALE_DEFAULT);
    expect(loadScale(storageWith("abc"))).toBe(SCALE_DEFAULT);
    expect(loadScale(storageWith(""))).toBe(SCALE_DEFAULT);
  });

  it("存的值越界（手改过、版本变过）：夹进范围，不把界面缩成看不见", () => {
    expect(loadScale(storageWith("0.05"))).toBe(SCALE_MIN);
    expect(loadScale(storageWith("40"))).toBe(SCALE_MAX);
    expect(loadScale(storageWith("-3"))).toBe(SCALE_MIN);
  });

  it("存储本身会抛（隐私模式、被禁用）：读不抛、写不抛，缩放这一轮照样能用", () => {
    const throwing = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
    };
    expect(loadScale(throwing)).toBe(SCALE_DEFAULT);
    expect(() => saveScale(1.2, throwing)).not.toThrow();
  });

  it("没有存储（node、非浏览器环境）：读回默认，写什么也不做", () => {
    expect(loadScale(undefined)).toBe(SCALE_DEFAULT);
    expect(() => saveScale(1.2, undefined)).not.toThrow();
  });
});
