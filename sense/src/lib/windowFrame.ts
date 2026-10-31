/**
 * 窗口边缘 / 角上拖着缩放的纯逻辑。
 *
 * 系统对无边框窗口自带缩放，但那个缩放区只有窗口最外缘约 3px（落在里面的按下由 AppKit 先处理，网页收不到），
 * 而可见的面板比窗口缩进 12px，用户抓可见的边框什么也不会发生；tao 0.37 的 `drag_resize_window` 在 macOS 上
 * 又是 NotSupported（调用静默无效），没法从前端发起系统缩放。所以自己跟踪指针：按下时问 Rust 要一次窗口外框，
 * 之后每次移动用这里的函数算出新外框，再通过 `set_window_frame` 一次设好位置和大小。
 */

/** 窗口外框，逻辑像素，原点在屏幕左上角（与 PointerEvent 的 screenX / screenY 同一个坐标系） */
export type Frame = { x: number; y: number; width: number; height: number };

/** 八个方向：n 上、s 下、e 右、w 左，两个字母是角 */
export type Edge = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

/** 与 tauri.conf.json 的 minWidth / minHeight、Rust window.rs 的 MIN_* 保持一致（有测试对拍） */
export const MIN_WIDTH = 760;
export const MIN_HEIGHT = 720;
/** 与 Rust window.rs 的 MAX_SIDE 一致：Rust 那边会再夹一次 */
export const MAX_SIDE = 8192;

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

/**
 * 从起始外框出发，把 `edge` 方向的边 / 角拖动 (dx, dy) 之后的新外框。
 *
 * 拖右边 / 下边只改宽高；拖左边 / 上边，对面那条边要纹丝不动，所以位置按「夹过之后的宽高」反推——
 * 缩到最小尺寸以后再往里拖，窗口不会被推着跑。
 */
export function resizeFrame(start: Frame, edge: Edge, dx: number, dy: number): Frame {
  let width = start.width;
  let height = start.height;
  if (edge.includes("e")) width = start.width + dx;
  if (edge.includes("w")) width = start.width - dx;
  if (edge.includes("s")) height = start.height + dy;
  if (edge.includes("n")) height = start.height - dy;

  width = clamp(Math.round(width), MIN_WIDTH, MAX_SIDE);
  height = clamp(Math.round(height), MIN_HEIGHT, MAX_SIDE);

  return {
    x: edge.includes("w") ? start.x + start.width - width : start.x,
    y: edge.includes("n") ? start.y + start.height - height : start.y,
    width,
    height,
  };
}

/**
 * 只留最新值的发送器：上一次还没发完时，新的外框只是覆盖「待发」，发完立刻接着发最新的。
 *
 * 指针移动的频率远高于一次 IPC 往返；每次都发会排起长队，松手以后窗口还在追旧位置。
 * 发送失败（窗口已经没了之类）吞掉，别让拖动卡死。
 */
export function createFrameSender(send: (frame: Frame) => Promise<unknown>) {
  let pending: Frame | null = null;
  let busy = false;

  const pump = async () => {
    if (busy) return;
    busy = true;
    try {
      while (pending) {
        const next = pending;
        pending = null;
        try {
          await send(next);
        } catch {
          /* 窗口被关掉时会失败，忽略 */
        }
      }
    } finally {
      busy = false;
    }
  };

  return {
    push(frame: Frame) {
      pending = frame;
      void pump();
    },
  };
}

/** 一次拖拽缩放的会话 */
export type DragSession = {
  /** 指针移动。`buttons` 是 PointerEvent.buttons：主键已经松开就收尾 */
  move(screenX: number, screenY: number, buttons: number): void;
  /** 收尾（松手、取消、失焦、指针捕获丢失……）；多次调用只生效一次 */
  end(): void;
  readonly ended: boolean;
};

/**
 * 把一次拖拽的时序收在一处，才能单测：按下 → 量起始外框（异步）→ 跟着指针发新外框 → 收尾。
 *
 * - 起始外框是异步量的：量完之前的移动先不动窗口；量的时候已经松手的，量完也不开始。
 *   （用户快速点一下就松手时 pointerup 发生在量的期间——旧实现栽过这个，监听器永远收不到松手。）
 * - 松手可能丢——窗口被快捷键 / Esc 隐藏、右键菜单打断、指针捕获被抢走：移动事件里主键已经松开就收尾，
 *   不然监听器残留，窗口再出现后没按着键移动鼠标也会缩放。
 * - 量不出来就放弃这次拖动。
 * `onEnd` 只调用一次，调用方在里面摘监听器、释放指针捕获。
 */
export function createDragSession(options: {
  edge: Edge;
  startX: number;
  startY: number;
  measure: () => Promise<Frame>;
  send: (frame: Frame) => Promise<unknown>;
  onEnd: () => void;
}): DragSession {
  let start: Frame | null = null;
  let ended = false;
  const sender = createFrameSender(options.send);

  const end = () => {
    if (ended) return;
    ended = true;
    options.onEnd();
  };

  try {
    options.measure().then(
      (frame) => {
        start = frame; // 量的时候已经松手的，move 一进来就因为 ended 返回，这个值用不上
      },
      () => end(),
    );
  } catch {
    end();
  }

  return {
    get ended() {
      return ended;
    },
    end,
    move(screenX, screenY, buttons) {
      if (ended) return;
      if ((buttons & 1) === 0) {
        end();
        return;
      }
      if (!start) return; // 还没量完起始外框，先不动
      sender.push(resizeFrame(start, options.edge, screenX - options.startX, screenY - options.startY));
    },
  };
}
