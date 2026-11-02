import { useCallback, useEffect, useRef, useState } from "react";
import { api, onUpdateStatus } from "../../lib/ipc";
import type { UpdateStatus } from "../../lib/updateStatus";
import { actionErrorExpired, alertToLatch, shouldAccept } from "./describe";

// 测试在 node 环境里导入这个文件，没有 window
export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** 哪个动作失败了：横幅只关心「重启」，设置页三个都显示 */
export type ActionSource = "check" | "restart" | "toggle";
export type ActionError = { source: ActionSource; message: string };

/** 更新相关的状态与动作，App 里建一份，横幅和设置页共用。 */
export type UpdateController = {
  /** null = 还没取到（或不在 Tauri 里，比如浏览器里看界面） */
  status: UpdateStatus | null;
  /** 点了按钮、等命令返回期间（下载很久，所以「检查更新」整段都是 busy）；有几个命令在途都算 */
  busy: boolean;
  /** 命令本身失败的原因（装不上、写不了设置……）；下一次操作前清掉，之后来了更新的状态也清掉 */
  actionError: ActionError | null;
  /** 状态取不到 / 认不出来（前后端版本对不上）的原因；取到一次合法的状态就清掉 */
  loadError: string | null;
  /**
   * 锁存的安装失败（需要人处理的那种：旧版本没放回原位、新版本装好了却没能拉起来）：
   * 从「正在安装」直接落到失败的那一刻记下，之后的后台检查把状态里的 `last_error` 清掉 / 覆盖也不影响它，
   * 用户点「知道了」（`dismissAlert`）或开始下一次安装才放掉。横幅靠它保证这种消息不会无声消失。
   */
  alert: string | null;
  dismissAlert: () => void;
  /** 用户明确看过、不想再看到的那条命令失败原因（横幅的「稍后」） */
  clearActionError: () => void;
  check: () => void;
  restart: () => void;
  setAutoUpdate: (enabled: boolean) => void;
};

/** Tauri 的错误是字符串，经 `String()` 会带一个 `Error:` 前缀 */
export function cleanError(err: unknown): string {
  return String(err).replace(/^Error:\s*/, "");
}

export function useUpdate(): UpdateController {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<ActionError | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [alert, setAlert] = useState<string | null>(null);
  // 已采纳的最大序号：命令的返回值和事件会乱序到达，比已采纳的旧的一律丢掉
  const newest = useRef(-1);
  // 命令失败那一刻已采纳的最大序号：之后再来更新的状态，就把那条失败原因撤掉，别一直挂着
  const errorAt = useRef(-1);
  // 在途的命令数：连点两下时，先返回的那个不能把 busy 关掉
  const pending = useRef(0);
  // 上一个已采纳的状态是不是「正在安装」：失败要不要锁存，只看「从安装直接落到失败」这一个转换
  const wasInstalling = useRef(false);

  const accept = useCallback((next: UpdateStatus) => {
    if (!shouldAccept(newest.current, next.seq)) return;
    newest.current = next.seq;
    setStatus(next);
    setLoadError(null);
    if (actionErrorExpired(errorAt.current, next.seq)) setActionError(null);
    // 新一次安装开始了：之前锁存的失败作废；从安装直接落到「失败、没有包可重试」：锁存它
    if (next.phase.state === "installing") setAlert(null);
    const latched = alertToLatch(wasInstalling.current, next);
    if (latched !== null) setAlert(latched);
    wasInstalling.current = next.phase.state === "installing";
  }, []);

  useEffect(() => {
    if (!isTauri) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;

    const fetchStatus = () => {
      // 请求发出时已采纳的最大序号：失败回来时要是已经采纳了更新的状态，这次失败就是过去的事，别再挂红字
      const startedAt = newest.current;
      api
        .updateStatus()
        .then((next) => {
          if (!disposed) accept(next);
        })
        .catch((err) => {
          // 取不到就说取不到：整块更新界面悄悄消失，用户不知道出了什么事
          if (!disposed && newest.current === startedAt) setLoadError(cleanError(err));
        });
    };

    // 订阅成功「之后」才取：订阅是异步生效的，两个同时发出去的话，取到状态之后、订阅生效之前发生的变化就漏了。
    // 订阅本身失败也要取一次，至少有个初始状态
    onUpdateStatus(accept, fetchStatus)
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {})
      .then(fetchStatus);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [accept]);

  const run = useCallback((source: ActionSource, task: () => Promise<unknown>) => {
    setActionError(null);
    pending.current += 1;
    setBusy(true);
    task()
      .catch((err) => {
        errorAt.current = newest.current;
        setActionError({ source, message: cleanError(err) });
      })
      .finally(() => {
        pending.current -= 1;
        setBusy(pending.current > 0);
      });
  }, []);

  return {
    status,
    busy,
    actionError,
    loadError,
    alert,
    dismissAlert: useCallback(() => setAlert(null), []),
    clearActionError: useCallback(() => setActionError(null), []),
    check: useCallback(() => run("check", async () => accept(await api.checkUpdate())), [run, accept]),
    restart: useCallback(() => run("restart", () => api.restartForUpdate()), [run]),
    setAutoUpdate: useCallback(
      (enabled: boolean) => run("toggle", async () => accept(await api.setAutoUpdate(enabled))),
      [run, accept],
    ),
  };
}
