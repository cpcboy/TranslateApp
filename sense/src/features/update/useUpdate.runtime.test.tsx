// 这个项目的测试跑在 node 环境、没有 DOM，也没有 react-testing-library：
// 这里用一个手写的最小 hooks 运行时（useState / useRef / useCallback / useEffect），
// 让 `useUpdate` 和 `UpdateBanner` 的真实代码跑起来，测的是它们的状态逻辑（乱序、重试、收起、监听的释放……），不是 React 本身。
// 两种用法：
// - 手动：每次 render 之后 `rt.effects` 里是这一轮登记的 effect，由测试自己决定跑不跑、跑几次（模拟严格模式）；
// - 自动（`renderCommit`）：像 React 一样，只有依赖（deps）变了的 effect 才先清理上一次、再重跑——
//   这样「依赖写错了（比如把 status 放进去），每个状态事件都取消订阅、重新订阅」这类回归才测得出来。
import type { ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { UpdateStatus } from "../../lib/updateStatus";

type Cell = { v?: unknown; current?: unknown; fn?: unknown; deps?: unknown[]; cleanup?: unknown };

const rt = vi.hoisted(() => {
  // useUpdate.ts 在导入时就判断 isTauri
  (globalThis as { window?: unknown }).window = { __TAURI_INTERNALS__: {} };
  return {
    hooks: [] as Cell[],
    idx: 0,
    effects: [] as Array<() => void | (() => void)>,
    /** 这一轮渲染里依赖变了、要在「提交」时跑的 effect（自动模式） */
    queue: [] as Array<() => void>,
  };
});

vi.mock("react", async (importOriginal) => {
  const original = await importOriginal<typeof import("react")>();
  return {
    ...original,
    useState: (init: unknown) => {
      const i = rt.idx++;
      if (!(i in rt.hooks)) rt.hooks[i] = { v: typeof init === "function" ? (init as () => unknown)() : init };
      const cell = rt.hooks[i];
      return [
        cell.v,
        (next: unknown) => {
          cell.v = typeof next === "function" ? (next as (prev: unknown) => unknown)(cell.v) : next;
        },
      ];
    },
    useRef: (init: unknown) => {
      const i = rt.idx++;
      if (!(i in rt.hooks)) rt.hooks[i] = { current: init };
      return rt.hooks[i];
    },
    useCallback: (fn: unknown, deps: unknown[]) => {
      const i = rt.idx++;
      const prev = rt.hooks[i];
      if (prev?.deps && prev.deps.length === deps.length && prev.deps.every((d, k) => Object.is(d, deps[k]))) return prev.fn;
      rt.hooks[i] = { fn, deps };
      return fn;
    },
    useEffect: (fn: () => void | (() => void), deps?: unknown[]) => {
      const i = rt.idx++;
      const cell: Cell = rt.hooks[i] ?? {};
      rt.hooks[i] = cell;
      rt.effects.push(fn);
      const changed = !cell.deps || !deps || cell.deps.length !== deps.length || deps.some((d, k) => !Object.is(d, cell.deps?.[k]));
      if (changed) {
        rt.queue.push(() => {
          (cell.cleanup as (() => void) | undefined)?.();
          cell.cleanup = fn();
          cell.deps = deps;
        });
      }
    },
  };
});

const win = vi.hoisted(() => ({
  handlers: [] as Array<(event: { payload: boolean }) => void>,
  unlistenCalls: 0,
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onFocusChanged: (handler: (event: { payload: boolean }) => void) => {
      win.handlers.push(handler);
      return Promise.resolve(() => {
        win.unlistenCalls++;
        win.handlers = win.handlers.filter((item) => item !== handler);
      });
    },
  }),
}));

const ipc = vi.hoisted(() => ({
  api: {
    updateStatus: vi.fn(),
    checkUpdate: vi.fn(),
    restartForUpdate: vi.fn(),
    setAutoUpdate: vi.fn(),
  },
  listen: {
    log: [] as string[],
    /** 订阅要等这个 promise 才算生效，模拟订阅是异步的 */
    gate: null as Promise<void> | null,
    fail: false,
    handlers: [] as Array<(status: unknown) => void>,
    invalid: [] as Array<() => void>,
    unlistenCalls: 0,
  },
}));
vi.mock("../../lib/ipc", () => ({
  api: ipc.api,
  onUpdateStatus: (handler: (status: unknown) => void, onInvalid?: () => void) => {
    ipc.listen.log.push("listen:requested");
    return (ipc.listen.gate ?? Promise.resolve()).then(() => {
      if (ipc.listen.fail) throw new Error("listen failed");
      ipc.listen.log.push("listen:resolved");
      // 每次订阅一个独立的登记，和 Tauri 的事件 id 一样
      const registration = (status: unknown) => handler(status);
      ipc.listen.handlers.push(registration);
      ipc.listen.invalid.push(() => onInvalid?.());
      return () => {
        ipc.listen.unlistenCalls++;
        ipc.listen.handlers = ipc.listen.handlers.filter((item) => item !== registration);
      };
    });
  },
}));

import fixture from "../../lib/updateStatus.fixture.json";
import { parseUpdateStatus } from "../../lib/updateStatus";
import { UpdateBanner } from "./UpdateBanner";
import { useUpdate, type UpdateController } from "./useUpdate";

const sample = (name: string, over: Partial<UpdateStatus> = {}): UpdateStatus => {
  const status = parseUpdateStatus((fixture as Record<string, unknown>)[name]);
  if (!status) throw new Error(`夹具 ${name} 解析不了`);
  return { ...status, ...over };
};

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

/** 挂载 useUpdate：render() 相当于一次重新渲染，返回这一轮的控制器；renderCommit() 还会像 React 那样提交 effect */
function mountHook() {
  rt.hooks = [];
  rt.idx = 0;
  rt.effects = [];
  rt.queue = [];
  return {
    render(): UpdateController {
      rt.idx = 0;
      rt.effects = [];
      rt.queue = [];
      return useUpdate();
    },
    renderCommit(): UpdateController {
      rt.idx = 0;
      rt.effects = [];
      rt.queue = [];
      const controller = useUpdate();
      rt.queue.forEach((run) => run());
      return controller;
    },
  };
}

/** 跑一次 effect，返回它的清理函数 */
const runEffect = (index = 0) => rt.effects[index]() as () => void;

const emit = (status: UpdateStatus) => ipc.listen.handlers.forEach((handler) => handler(status));

beforeEach(() => {
  ipc.listen.log = [];
  ipc.listen.gate = null;
  ipc.listen.fail = false;
  ipc.listen.handlers = [];
  ipc.listen.invalid = [];
  ipc.listen.unlistenCalls = 0;
  win.handlers = [];
  win.unlistenCalls = 0;
  for (const fn of Object.values(ipc.api)) fn.mockReset();
});

describe("useUpdate（真实代码，手写的 hooks 运行时）", () => {
  it("采纳状态：序号比已采纳的最大序号旧的丢掉，相同或更新的采纳", async () => {
    ipc.api.updateStatus.mockResolvedValue(sample("idle", { seq: 3 }));
    const hook = mountHook();
    hook.render();
    runEffect();
    await flush();
    expect(hook.render().status?.seq).toBe(3);
    emit(sample("downloading", { seq: 9 }));
    expect(hook.render().status?.seq).toBe(9);
    emit(sample("checking", { seq: 8 }));
    expect(hook.render().status?.seq).toBe(9); // 晚到的旧状态丢掉
    emit(sample("ready", { seq: 9 }));
    expect(hook.render().status?.phase.state).toBe("ready"); // 相同序号采纳
  });

  it("严格模式下 effect 跑两遍：最后只剩一个监听，多出来的那个被取消", async () => {
    ipc.api.updateStatus.mockResolvedValue(sample("idle", { seq: 1 }));
    const hook = mountHook();
    hook.render();
    const first = runEffect();
    first(); // 严格模式的第一次清理
    const second = runEffect();
    await flush();
    expect(ipc.listen.handlers.length).toBe(1);
    expect(ipc.listen.unlistenCalls).toBe(1);
    second();
    expect(ipc.listen.handlers.length).toBe(0);
  });

  it("先订阅、订阅生效之后才取状态（同时发出去的话，两者之间发生的变化会漏）", async () => {
    let open!: () => void;
    ipc.listen.gate = new Promise<void>((resolve) => {
      open = resolve;
    });
    ipc.api.updateStatus.mockImplementation(() => {
      ipc.listen.log.push("status:requested");
      return Promise.resolve(sample("idle", { seq: 1 }));
    });
    const hook = mountHook();
    hook.render();
    runEffect();
    await flush();
    expect(ipc.listen.log).toEqual(["listen:requested"]); // 订阅还没生效，状态还没去取
    open();
    await flush();
    expect(ipc.listen.log).toEqual(["listen:requested", "listen:resolved", "status:requested"]);
    expect(hook.render().status?.seq).toBe(1);
  });

  it("订阅本身失败也要取一次状态：至少有个初始状态", async () => {
    ipc.listen.fail = true;
    ipc.api.updateStatus.mockResolvedValue(sample("up_to_date", { seq: 4 }));
    const hook = mountHook();
    hook.render();
    runEffect();
    await flush();
    expect(ipc.api.updateStatus).toHaveBeenCalledTimes(1);
    expect(hook.render().status?.phase.state).toBe("up_to_date");
  });

  it("状态取不到（命令失败 / 格式不认识）：loadError 说出原因；之后来了合法的状态就清掉", async () => {
    ipc.api.updateStatus.mockRejectedValue(new Error("更新状态的格式不认识（前后端版本对不上？）"));
    const hook = mountHook();
    hook.render();
    runEffect();
    await flush();
    const failed = hook.render();
    expect(failed.status).toBeNull();
    expect(failed.loadError).toBe("更新状态的格式不认识（前后端版本对不上？）");
    emit(sample("idle", { seq: 2 }));
    const recovered = hook.render();
    expect(recovered.status?.seq).toBe(2);
    expect(recovered.loadError).toBeNull();
  });

  it("事件载荷格式不对：重新取一次，取回来是好的就悄悄恢复，取不到就报 loadError", async () => {
    ipc.api.updateStatus.mockResolvedValueOnce(sample("idle", { seq: 1 }));
    const hook = mountHook();
    hook.render();
    runEffect();
    await flush();
    expect(ipc.api.updateStatus).toHaveBeenCalledTimes(1);

    ipc.api.updateStatus.mockResolvedValueOnce(sample("up_to_date", { seq: 2 }));
    ipc.listen.invalid[0]();
    await flush();
    expect(ipc.api.updateStatus).toHaveBeenCalledTimes(2);
    expect(hook.render().status?.seq).toBe(2);
    expect(hook.render().loadError).toBeNull();

    ipc.api.updateStatus.mockRejectedValueOnce("更新状态的格式不认识（前后端版本对不上？）");
    ipc.listen.invalid[0]();
    await flush();
    const broken = hook.render();
    expect(broken.loadError).toContain("前后端版本对不上");
    expect(broken.status?.seq).toBe(2); // 手里的旧状态还在，不清掉
  });

  it("连点两次检查：命令不被挡（按钮靠 disabled 防连点），先返回的那个不能把 busy 关掉", async () => {
    let first!: (status: UpdateStatus) => void;
    let second!: (status: UpdateStatus) => void;
    ipc.api.checkUpdate
      .mockImplementationOnce(() => new Promise((resolve) => (first = resolve)))
      .mockImplementationOnce(() => new Promise((resolve) => (second = resolve)));
    const hook = mountHook();
    const update = hook.render();
    update.check();
    update.check();
    expect(ipc.api.checkUpdate).toHaveBeenCalledTimes(2);
    expect(hook.render().busy).toBe(true);
    first(sample("up_to_date", { seq: 5 }));
    await flush();
    expect(hook.render().busy).toBe(true); // 第二个还在途
    second(sample("up_to_date", { seq: 6 }));
    await flush();
    expect(hook.render().busy).toBe(false);
  });

  it("重启被拒绝：actionError 带来源、去掉 Error: 前缀，busy 恢复；再点一次成功就清掉", async () => {
    ipc.api.restartForUpdate.mockRejectedValueOnce(new Error("没能替换「应用程序」里的 SENSE：可能是你取消了管理员授权"));
    const hook = mountHook();
    const update = hook.render();
    update.restart();
    expect(hook.render().busy).toBe(true);
    await flush();
    const after = hook.render();
    expect(after.busy).toBe(false);
    expect(after.actionError).toEqual({ source: "restart", message: "没能替换「应用程序」里的 SENSE：可能是你取消了管理员授权" });
    ipc.api.restartForUpdate.mockResolvedValueOnce(undefined);
    after.restart();
    await flush();
    const ok = hook.render();
    expect(ok.actionError).toBeNull();
    expect(ok.busy).toBe(false);
  });

  it("检查 / 开关失败也带各自的来源（横幅只认「重启」）", async () => {
    ipc.api.checkUpdate.mockRejectedValueOnce("检查炸了");
    ipc.api.setAutoUpdate.mockRejectedValueOnce("写不进设置文件");
    const hook = mountHook();
    hook.render().check();
    await flush();
    expect(hook.render().actionError).toEqual({ source: "check", message: "检查炸了" });
    hook.render().setAutoUpdate(false);
    await flush();
    expect(hook.render().actionError).toEqual({ source: "toggle", message: "写不进设置文件" });
  });

  it("命令失败的原因：来了更新的状态才撤，同一序号的不撤", async () => {
    ipc.api.setAutoUpdate.mockRejectedValueOnce("写不进设置文件");
    ipc.api.updateStatus.mockResolvedValue(sample("up_to_date", { seq: 20 }));
    const hook = mountHook();
    const update = hook.render();
    runEffect();
    await flush(); // 已采纳的最大序号 = 20
    update.setAutoUpdate(false);
    await flush();
    expect(hook.render().actionError?.message).toBe("写不进设置文件");
    emit(sample("up_to_date", { seq: 20 }));
    expect(hook.render().actionError?.message).toBe("写不进设置文件");
    emit(sample("checking", { seq: 21 }));
    expect(hook.render().actionError).toBeNull();
  });

  it("命令的拒绝比 Rust 失败时发的那个状态事件先到：命令的错误会被后到的事件撤掉，但状态里自己带着同一句，没有丢", async () => {
    // Rust 的顺序：emit(installing, 11) → emit(ready + last_error, 12) → 返回 Err；前端收到的顺序可能是 11、拒绝、12
    const message = "没能替换「应用程序」里的 SENSE：可能是你取消了管理员授权";
    ipc.api.updateStatus.mockResolvedValue(sample("ready", { seq: 10 }));
    let reject!: (reason: unknown) => void;
    ipc.api.restartForUpdate.mockImplementation(() => new Promise((_, rej) => (reject = rej)));
    const hook = mountHook();
    const update = hook.render();
    runEffect();
    await flush();
    update.restart();
    emit(sample("installing", { seq: 11 }));
    reject(message);
    await flush();
    expect(hook.render().actionError?.message).toBe(message);
    emit(sample("ready", { seq: 12, last_error: message, error_kind: "install" }));
    const after = hook.render();
    expect(after.actionError).toBeNull();
    expect(after.status?.last_error).toBe(message);
    expect(after.status?.error_kind).toBe("install"); // 横幅据此照样画出这句话
  });

  it("clearActionError 撤掉命令失败的原因", async () => {
    ipc.api.restartForUpdate.mockRejectedValueOnce("授权被取消了");
    const hook = mountHook();
    hook.render().restart();
    await flush();
    expect(hook.render().actionError?.message).toBe("授权被取消了");
    hook.render().clearActionError();
    expect(hook.render().actionError).toBeNull();
  });

  it("开关：把值原样传下去，并采纳命令返回的状态", async () => {
    ipc.api.setAutoUpdate.mockResolvedValue(sample("up_to_date", { seq: 31, auto_update: false }));
    const hook = mountHook();
    hook.render().setAutoUpdate(false);
    await flush();
    expect(ipc.api.setAutoUpdate).toHaveBeenCalledWith(false);
    expect(hook.render().status?.auto_update).toBe(false);
  });

  it("检查：采纳命令返回的状态（即使一个事件都没收到）；失败后 busy 也恢复", async () => {
    ipc.api.checkUpdate.mockResolvedValueOnce(sample("up_to_date", { seq: 40 }));
    const hook = mountHook();
    hook.render().check();
    await flush();
    expect(hook.render().status?.seq).toBe(40);
    expect(hook.render().busy).toBe(false);
    ipc.api.checkUpdate.mockRejectedValueOnce("boom");
    hook.render().check();
    expect(hook.render().busy).toBe(true);
    await flush();
    expect(hook.render().busy).toBe(false);
  });

  it("卸载时取消已经登记的订阅", async () => {
    ipc.api.updateStatus.mockResolvedValue(sample("idle", { seq: 1 }));
    const hook = mountHook();
    hook.render();
    const cleanup = runEffect();
    await flush();
    expect(ipc.listen.handlers.length).toBe(1);
    cleanup();
    expect(ipc.listen.unlistenCalls).toBe(1);
  });

  it("卸载之后才到的状态 / 失败不再写进 state", async () => {
    let resolve!: (status: UpdateStatus) => void;
    ipc.api.updateStatus.mockImplementation(() => new Promise((res) => (resolve = res)));
    const hook = mountHook();
    hook.render();
    const cleanup = runEffect();
    await flush();
    cleanup();
    resolve(sample("idle", { seq: 5 }));
    await flush();
    expect(hook.render().status).toBeNull();
  });
});

describe("失败锁存（真实代码）", () => {
  const message = "安装中途失败，而且旧版本没能放回原位。旧版本在 /Applications/SENSE (previous version).app，请手动把它拖回「应用程序」文件夹";

  it("安装直接落到失败：锁存；之后的后台检查成功 / 失败把状态里的错误清掉 / 覆盖也不影响；「知道了」才放掉", async () => {
    ipc.api.updateStatus.mockResolvedValue(sample("ready", { seq: 10 }));
    const hook = mountHook();
    hook.render();
    runEffect();
    await flush();
    emit(sample("installing", { seq: 11 }));
    expect(hook.render().alert).toBeNull();
    emit(sample("failed_install", { seq: 12, last_error: message }));
    expect(hook.render().alert).toBe(message);
    emit(sample("up_to_date", { seq: 13 })); // 后台检查成功：状态里的错误清了
    expect(hook.render().status?.last_error).toBeNull();
    expect(hook.render().alert).toBe(message);
    emit(sample("failed_check", { seq: 14 })); // 后台检查失败：last_error 被检查类的错误覆盖
    expect(hook.render().alert).toBe(message);
    hook.render().dismissAlert();
    expect(hook.render().alert).toBeNull();
  });

  it("回到已就绪（可重试的失败：取消了授权，包还在）不锁", async () => {
    ipc.api.updateStatus.mockResolvedValue(sample("ready", { seq: 10 }));
    const hook = mountHook();
    hook.render();
    runEffect();
    await flush();
    emit(sample("installing", { seq: 11 }));
    emit(sample("ready_install_failed", { seq: 12 }));
    expect(hook.render().alert).toBeNull();
  });

  it("没经过「正在安装」就收到带安装错误的状态（重新取到的初始状态、下载别的版本时残留的错误）：不锁", async () => {
    ipc.api.updateStatus.mockResolvedValue(sample("failed_install", { seq: 3 }));
    const hook = mountHook();
    hook.render();
    runEffect();
    await flush();
    expect(hook.render().status?.error_kind).toBe("install");
    expect(hook.render().alert).toBeNull();
    emit(sample("downloading", { seq: 4, last_error: message, error_kind: "install" }));
    expect(hook.render().alert).toBeNull();
  });

  it("下一次安装开始，旧的锁存作废；晚到的旧状态不影响锁存", async () => {
    ipc.api.updateStatus.mockResolvedValue(sample("ready", { seq: 10 }));
    const hook = mountHook();
    hook.render();
    runEffect();
    await flush();
    emit(sample("installing", { seq: 11 }));
    emit(sample("failed_install", { seq: 12, last_error: message }));
    expect(hook.render().alert).toBe(message);
    emit(sample("installing", { seq: 11 })); // 晚到的旧事件：丢掉，锁存不变
    expect(hook.render().alert).toBe(message);
    emit(sample("ready", { seq: 13 }));
    emit(sample("installing", { seq: 14 })); // 用户又点了一次重启
    expect(hook.render().alert).toBeNull();
  });

  it("取状态的请求失败回来时，要是已经采纳了更新的状态，这次失败是过去的事：不再挂红字", async () => {
    let reject!: (reason: unknown) => void;
    ipc.api.updateStatus.mockImplementation(() => new Promise((_, rej) => (reject = rej)));
    const hook = mountHook();
    hook.render();
    runEffect();
    await flush();
    emit(sample("up_to_date", { seq: 5 })); // 事件先到
    reject("更新状态的格式不认识（前后端版本对不上？）");
    await flush();
    const after = hook.render();
    expect(after.loadError).toBeNull();
    expect(after.status?.seq).toBe(5);
  });
});

describe("effect 的依赖没被写坏（deps 敏感的运行时）", () => {
  it("useUpdate：连来 5 个状态事件、重新渲染 5 次之后，订阅和初次取状态都只发生一次，一次也没取消订阅", async () => {
    ipc.api.updateStatus.mockResolvedValue(sample("up_to_date", { seq: 1 }));
    const hook = mountHook();
    hook.renderCommit();
    await flush();
    for (let seq = 2; seq <= 6; seq += 1) {
      emit(sample("downloading", { seq }));
      hook.renderCommit();
      await flush();
    }
    expect(ipc.listen.log.filter((line) => line === "listen:requested")).toHaveLength(1);
    expect(ipc.api.updateStatus).toHaveBeenCalledTimes(1);
    expect(ipc.listen.unlistenCalls).toBe(0);
  });

  it("UpdateBanner：状态变了 5 次，窗口焦点监听仍然只登记一次，一次也没取消", async () => {
    let status = sample("ready", { seq: 1 });
    const controller = (): UpdateController => ({
      status,
      busy: false,
      actionError: null,
      loadError: null,
      alert: null,
      dismissAlert: vi.fn(),
      clearActionError: vi.fn(),
      check: vi.fn(),
      restart: vi.fn(),
      setAutoUpdate: vi.fn(),
    });
    rt.hooks = [];
    const draw = () => {
      rt.idx = 0;
      rt.effects = [];
      rt.queue = [];
      UpdateBanner({ update: controller() });
      rt.queue.forEach((run) => run());
    };
    draw();
    await flush();
    for (let seq = 2; seq <= 6; seq += 1) {
      status = sample("ready", { seq });
      draw();
      await flush();
    }
    expect(win.handlers.length).toBe(1);
    expect(win.unlistenCalls).toBe(0);
  });
});

describe("UpdateBanner 的状态逻辑（真实代码）", () => {
  const controller = (status: UpdateStatus | null, over: Partial<UpdateController> = {}): UpdateController => ({
    status,
    busy: false,
    actionError: null,
    loadError: null,
    alert: null,
    dismissAlert: vi.fn(),
    clearActionError: vi.fn(),
    check: vi.fn(),
    restart: vi.fn(),
    setAutoUpdate: vi.fn(),
    ...over,
  });

  /** 渲染 UpdateBanner：返回外壳里的横幅元素（没有横幅就是 false）；返回的函数上的 `commit` 版本还会提交 effect */
  const mountBanner = (getUpdate: () => UpdateController) => {
    rt.hooks = [];
    const draw = (commit: boolean) => {
      rt.idx = 0;
      rt.effects = [];
      rt.queue = [];
      const region = UpdateBanner({ update: getUpdate() }) as ReactElement<{ children: ReactElement<BannerProps> | false }>;
      if (commit) rt.queue.forEach((run) => run());
      return region.props.children;
    };
    return Object.assign(() => draw(false), { commit: () => draw(true) });
  };
  type BannerProps = { onRestart: () => void; onLater: () => void; model: { kind: string } };

  const focus = (payload: boolean) => win.handlers.forEach((handler) => handler({ payload }));

  it("「稍后」收起横幅；窗口重新拿到焦点（Tauri 原生事件）横幅回来，失焦不会", async () => {
    const render = mountBanner(() => controller(sample("ready")));
    const first = render();
    expect(first).toBeTruthy();
    const cleanup = runEffect();
    await flush();
    (first as ReactElement<BannerProps>).props.onLater();
    expect(render()).toBe(false);
    focus(false);
    expect(render()).toBe(false);
    focus(true);
    expect(render()).toBeTruthy();
    cleanup();
    expect(win.unlistenCalls).toBe(1);
  });

  it("出了更新的版本，收起的横幅再出现", () => {
    let status = sample("ready");
    const render = mountBanner(() => controller(status));
    (render() as ReactElement<BannerProps>).props.onLater();
    expect(render()).toBe(false);
    status = sample("ready", { phase: { state: "ready", version: "0.1.2" }, seq: 12 });
    expect(render()).toBeTruthy();
  });

  it("同一版装失败了（出了新的错误），收起的横幅再出现；带着错误时「稍后」照样收得起来", () => {
    let status = sample("ready");
    const render = mountBanner(() => controller(status));
    (render() as ReactElement<BannerProps>).props.onLater();
    expect(render()).toBe(false);
    status = sample("ready_install_failed");
    expect(render()).toBeTruthy(); // 新出的错误必须让人看见
    (render() as ReactElement<BannerProps>).props.onLater();
    expect(render()).toBe(false); // 带着错误也收得起来
  });

  it("正在安装的横幅，即使这一版点过「稍后」也要显示", () => {
    let status = sample("ready");
    const render = mountBanner(() => controller(status));
    (render() as ReactElement<BannerProps>).props.onLater();
    expect(render()).toBe(false);
    status = sample("installing", { seq: 11 });
    expect((render() as ReactElement<BannerProps>).props.model.kind).toBe("installing");
  });

  it("装失败而且包不能再试：状态回到空闲，横幅照样在（主界面看得见）；「知道了」收起后同一句错误不会马上又冒出来", () => {
    const render = mountBanner(() => controller(sample("failed_install")));
    const banner = render() as ReactElement<BannerProps>;
    expect(banner.props.model.kind).toBe("failed");
    banner.props.onLater();
    expect(render()).toBe(false);
  });

  it("锁存的失败：状态里的错误被后台检查清掉之后横幅仍在；「知道了」把锁存放掉并收起；「稍后」（已就绪那一档）不动锁存", () => {
    const message = "新版本已经装好，但没能自动重启（spawn failed）。请退出 SENSE 后重新打开";
    const dismissAlert = vi.fn();
    const stuck = mountBanner(() => controller(sample("up_to_date"), { alert: message, dismissAlert }));
    const banner = stuck() as ReactElement<BannerProps>;
    expect(banner.props.model).toMatchObject({ kind: "failed", error: message });
    banner.props.onLater();
    expect(dismissAlert).toHaveBeenCalledTimes(1);

    const notDismissed = vi.fn();
    const ready = mountBanner(() => controller(sample("ready"), { alert: message, dismissAlert: notDismissed }));
    (ready() as ReactElement<BannerProps>).props.onLater();
    expect(notDismissed).not.toHaveBeenCalled();
  });

  it("「稍后」会撤掉命令返回的重启失败原因；别的来源的失败原因不动", () => {
    const clearRestart = vi.fn();
    const restartFailed = controller(sample("ready"), {
      actionError: { source: "restart", message: "授权被取消了" },
      clearActionError: clearRestart,
    });
    mountBanner(() => restartFailed)();
    (mountBanner(() => restartFailed)() as ReactElement<BannerProps>).props.onLater();
    expect(clearRestart).toHaveBeenCalledTimes(1);

    const clearToggle = vi.fn();
    const toggleFailed = controller(sample("ready"), {
      actionError: { source: "toggle", message: "写不进设置文件" },
      clearActionError: clearToggle,
    });
    (mountBanner(() => toggleFailed)() as ReactElement<BannerProps>).props.onLater();
    expect(clearToggle).not.toHaveBeenCalled();
  });

  it("按钮接线穿过有状态的外壳：立即重启 → update.restart，不会误触别的动作", () => {
    const update = controller(sample("ready"));
    const banner = mountBanner(() => update)() as ReactElement<BannerProps>;
    banner.props.onRestart();
    expect(update.restart).toHaveBeenCalledTimes(1);
    expect(update.check).not.toHaveBeenCalled();
    expect(update.setAutoUpdate).not.toHaveBeenCalled();
  });

  it("监听窗口焦点的 effect：严格模式下只留一个，卸载时释放", async () => {
    const render = mountBanner(() => controller(sample("ready")));
    render();
    const effect = rt.effects[0];
    (effect() as () => void)(); // 严格模式：跑一遍立刻清理
    const second = effect() as () => void;
    await flush();
    expect(win.handlers.length).toBe(1);
    expect(win.unlistenCalls).toBe(1);
    second();
    expect(win.handlers.length).toBe(0);
  });

  it("状态没取到 / 这台不能更新：没有横幅", () => {
    expect(mountBanner(() => controller(null))()).toBe(false);
    expect(mountBanner(() => controller(sample("unavailable")))()).toBe(false);
  });
});
