/** 两栏同步滚动：只用到滚动容器的这三个字段，测试里可以用假对象代替 DOM。 */
export interface ScrollBox {
  scrollTop: number;
  readonly scrollHeight: number;
  readonly clientHeight: number;
}

/** 回响判定的容差：写入与事件读数之间允许差 1px（设备像素取整） */
const ECHO_TOLERANCE = 1;
/**
 * 回响的保质期，只是兜底：窗口被遮挡、主线程卡住，回响迟到超过它就不再认，
 * 代价至多一次 1px 的毛刺。设得太短会出事——帧间隔超过它，每个回响都迟到，
 * 写回循环就复发（200ms 时实测，每帧 ≥250ms 的卡顿下两栏照样漂）
 */
const ECHO_TTL_MS = 1000;

/**
 * 按比例把一栏的滚动位置带到另一栏。
 *
 * 写对方的 `scrollTop` 会让对方也触发 scroll 事件（下一帧才到）。以前靠「同一帧内互斥」
 * 挡这个回响，但事件到的时候互斥早已解除，对方就把它当成用户滚动、又按比例写回来；
 * WebKit 写 scrollTop 会取整，每个来回丢 1px，两栏就一直往顶部漂，用户停手了也停不下来。
 *
 * 现在记下每次写进去的位置：对方的下一个 scroll 事件如果还停在这个位置，就是我们自己的回响，
 * 直接忽略、不再往回写；位置对不上，说明用户又动了它，照常同步。回响不会引出新的写入，
 * 所以循环从结构上就不存在；判定靠位置，不靠帧时序（保质期只是兜底，见 ECHO_TTL_MS）。
 */
export function createScrollSync(now: () => number = () => performance.now()) {
  const echoes = new Map<ScrollBox, { top: number; at: number }>();

  return function sync(from: ScrollBox, to: ScrollBox): void {
    const echo = echoes.get(from);
    if (echo) {
      // 命中后刻意不删条目：一次写入之后可能连着来不止一个事件，都得认出来
      if (now() - echo.at <= ECHO_TTL_MS && Math.abs(from.scrollTop - echo.top) <= ECHO_TOLERANCE) return;
      echoes.delete(from); // 过期，或者用户在回响之前又动了它：这是真滚动
    }

    const ratio = from.scrollTop / Math.max(1, from.scrollHeight - from.clientHeight);
    const before = to.scrollTop;
    to.scrollTop = ratio * Math.max(0, to.scrollHeight - to.clientHeight);
    // 以写完之后读到的位置为准（取整、夹到边界后的实际值）；没动就不会有事件，也就不用记
    if (to.scrollTop !== before) echoes.set(to, { top: to.scrollTop, at: now() });
  };
}
