import { isValidElement, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import fixture from "../../lib/updateStatus.fixture.json";
import { parseUpdateStatus, type UpdateStatus } from "../../lib/updateStatus";

// IPC 换成假的：这里只看界面画了什么、按钮接到了哪个动作，不碰真的窗口 API
vi.mock("../../lib/ipc", () => ({ api: {}, onUpdateStatus: async () => () => {} }));

import { bannerModel, type BannerModel } from "./describe";
import { BannerView, READY_TEXT, UpdateBanner } from "./UpdateBanner";
import { UpdateCard } from "./UpdateCard";
import { cleanError, type UpdateController } from "./useUpdate";

const sample = (name: string, over: Partial<UpdateStatus> = {}): UpdateStatus => {
  const status = parseUpdateStatus((fixture as Record<string, unknown>)[name]);
  if (!status) throw new Error(`夹具 ${name} 解析不了`);
  return { ...status, ...over };
};

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

/** 深度优先走一遍元素树（只走 props.children），把每个元素交给 visit */
function walk(node: ReactNode, visit: (element: ReactElement<Record<string, unknown>>) => void) {
  if (Array.isArray(node)) {
    node.forEach((child) => walk(child, visit));
    return;
  }
  if (!isValidElement(node)) return;
  const element = node as ReactElement<Record<string, unknown>>;
  visit(element);
  walk(element.props.children as ReactNode, visit);
}

const textOf = (node: ReactNode): string => {
  let out = "";
  const collect = (value: ReactNode) => {
    if (Array.isArray(value)) value.forEach(collect);
    else if (typeof value === "string" || typeof value === "number") out += String(value);
    else if (isValidElement(value)) collect((value.props as { children?: ReactNode }).children);
  };
  collect(node);
  return out;
};

type ButtonProps = {
  onClick: () => void;
  disabled?: boolean;
  role?: string;
  "aria-checked"?: boolean;
  "aria-label"?: string;
};
type Button = { label: string; props: ButtonProps };
const buttons = (tree: ReactNode): Button[] => {
  const found: Button[] = [];
  walk(tree, (element) => {
    if (element.type === "button") {
      found.push({ label: textOf(element.props.children as ReactNode).trim(), props: element.props as unknown as ButtonProps });
    }
  });
  return found;
};
const labels = (tree: ReactNode) => buttons(tree).map((item) => item.label);
const button = (tree: ReactNode, label: string): ButtonProps => {
  const hit = buttons(tree).find((item) => item.label.includes(label));
  if (!hit) throw new Error(`找不到按钮「${label}」，现有：${labels(tree).join(" / ")}`);
  return hit.props;
};

const model = (name: string): BannerModel => {
  const found = bannerModel(sample(name));
  if (!found) throw new Error(`夹具 ${name} 不该有横幅`);
  return found;
};
type BannerProps = Parameters<typeof BannerView>[0];
const view = (kind: BannerModel["kind"], over: Partial<BannerProps> = {}) =>
  BannerView({
    model: { kind, version: kind === "failed" ? "" : "0.1.1", error: null },
    busy: false,
    extraError: null,
    onRestart: vi.fn(),
    onLater: vi.fn(),
    ...over,
  });

const EMPTY_REGION = '<div role="status" aria-live="polite" class="shrink-0"></div>';

describe("顶部横幅", () => {
  it("已下载好时出现：写明版本，说清点哪里、不点会怎样，有「立即重启」和「稍后」", () => {
    const html = renderToStaticMarkup(<UpdateBanner update={controller(sample("ready"))} />);
    expect(html).toContain("v0.1.1");
    expect(html).toContain("立即重启");
    expect(html).toContain("稍后");
    expect(html).toContain("中断正在进行的翻译");
    expect(html).toContain("自己退出再打开不会更新");
    expect(READY_TEXT).toContain("自己退出再打开不会更新");
  });

  it("外层的 status 区域一直在（读屏软件只播报已经存在的 live region 里后来出现的内容）：有横幅时它包着横幅，没有时是个空壳", () => {
    expect(renderToStaticMarkup(<UpdateBanner update={controller(sample("ready"))} />)).toMatch(
      /^<div role="status" aria-live="polite" class="shrink-0"><div /,
    );
    expect(renderToStaticMarkup(<UpdateBanner update={controller(sample("up_to_date"))} />)).toBe(EMPTY_REGION);
  });

  it("安装中横幅不消失：改说「正在安装」，只剩一个置灰的「正在安装…」（弹管理员授权那几秒窗口里不能什么提示都没有）", () => {
    const html = renderToStaticMarkup(<UpdateBanner update={controller(sample("installing"))} />);
    expect(html).toContain("正在安装");
    expect(html).toContain("v0.1.1");
    expect(html).not.toContain("已下载好");
    expect(html).not.toContain("稍后");
    expect((html.match(/<button/g) ?? []).length).toBe(1);
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>正在安装…<\/button>/);
  });

  it("安装 / 重启没成而且包已经不能再试：在主界面说清楚，只有「知道了」，没有「立即重启」", () => {
    const html = renderToStaticMarkup(<UpdateBanner update={controller(sample("failed_install"))} />);
    expect(html).toContain("更新需要你处理");
    expect(html).not.toContain("更新没有完成"); // 有时新版本其实已经装好了，只是没能自己重新打开：别说「没有完成」
    expect(html).toContain("旧版本没能放回原位");
    expect(html).toContain("知道了");
    expect(html).not.toContain("立即重启");
    expect(html).not.toContain("稍后");
  });

  it("锁存的失败：状态里的错误已经被后台检查清掉了，横幅照样在，原因还是那句", () => {
    const message = "新版本已经装好，但没能自动重启（spawn failed）。请退出 SENSE 后重新打开";
    const html = renderToStaticMarkup(<UpdateBanner update={controller(sample("up_to_date"), { alert: message })} />);
    expect(html).toContain("更新需要你处理");
    expect(html).toContain(message);
    expect(html).toContain("知道了");
  });

  it("包还在、只是这次安装没成（取消了授权）：仍是「立即重启」，原因摆出来，同一句只出现一遍", () => {
    const status = sample("ready_install_failed");
    const message = status.last_error ?? "";
    const html = renderToStaticMarkup(
      <UpdateBanner update={controller(status, { actionError: { source: "restart", message } })} />,
    );
    expect(html).toContain("立即重启");
    expect(html.split(message).length - 1).toBe(1);
  });

  it("没有横幅的阶段只留空壳：检查中 / 下载中 / 检查失败 / 这台不能更新 / 状态没取到", () => {
    for (const name of ["idle", "checking", "up_to_date", "downloading", "failed_check", "unavailable"]) {
      expect(renderToStaticMarkup(<UpdateBanner update={controller(sample(name))} />), name).toBe(EMPTY_REGION);
    }
    expect(renderToStaticMarkup(<UpdateBanner update={controller(null)} />)).toBe(EMPTY_REGION);
  });

  it("按钮接线：立即重启 → onRestart，稍后 → onLater，互不串线", () => {
    const onRestart = vi.fn();
    const onLater = vi.fn();
    const tree = view("ready", { onRestart, onLater });
    button(tree, "立即重启").onClick();
    expect(onRestart).toHaveBeenCalledTimes(1);
    expect(onLater).not.toHaveBeenCalled();
    button(tree, "稍后").onClick();
    expect(onLater).toHaveBeenCalledTimes(1);
    expect(onRestart).toHaveBeenCalledTimes(1);
  });

  it("失败横幅的「知道了」接到 onLater", () => {
    const onLater = vi.fn();
    const tree = view("failed", { onLater });
    expect(labels(tree)).toEqual(["知道了"]);
    button(tree, "知道了").onClick();
    expect(onLater).toHaveBeenCalledTimes(1);
  });

  it("按钮文字跟着实际在做的事走：安装中是「正在安装…」；只是复查 / 开关在途时是「请稍候…」，不能说成在安装", () => {
    const installing = view("installing", { busy: true });
    expect(labels(installing)).toEqual(["正在安装…"]);
    expect(button(installing, "正在安装").disabled).toBe(true);

    const waiting = view("ready", { busy: true });
    expect(labels(waiting)).toEqual(["请稍候…", "稍后"]);
    expect(button(waiting, "请稍候").disabled).toBe(true);
    expect(button(waiting, "稍后").disabled).toBe(true);

    const idle = view("ready");
    expect(labels(idle)).toEqual(["立即重启", "稍后"]);
    expect(button(idle, "立即重启").disabled).toBe(false);
    expect(button(idle, "稍后").disabled).toBe(false);
  });

  it("「立即重启」这个动作本身失败了：原因补在横幅里；检查 / 开关失败的原因不上横幅", () => {
    const restart = renderToStaticMarkup(
      <UpdateBanner update={controller(sample("ready"), { actionError: { source: "restart", message: "正在下载更新 v0.1.2，下载完再点" } })} />,
    );
    expect(restart).toContain("正在下载更新 v0.1.2");
    for (const source of ["check", "toggle"] as const) {
      const html = renderToStaticMarkup(
        <UpdateBanner update={controller(sample("ready"), { actionError: { source, message: "写不进设置" } })} />,
      );
      expect(html, source).not.toContain("写不进设置");
    }
  });

  it("横幅的画法完全由状态决定：夹具里每个有横幅的状态都画得出来", () => {
    for (const name of ["ready", "installing", "failed_install", "ready_install_failed"]) {
      const html = renderToStaticMarkup(<UpdateBanner update={controller(sample(name))} />);
      expect(html, name).toContain("<button");
      expect(model(name).kind, name).toBeTruthy();
    }
  });
});

describe("设置页的「更新」卡片", () => {
  it("显示当前版本、状态、最近一次更新和下载来源", () => {
    const html = renderToStaticMarkup(<UpdateCard update={controller(sample("ready"))} />);
    expect(html).toContain("当前 v0.1.0");
    expect(html).toContain("v0.1.1 已下载并通过签名校验");
    expect(html).toContain("最近一次更新：v0.0.9 → v0.1.0");
    expect(html).toContain("GitHub");
  });

  it("最近一次更新不是当前这一版（手动降级过）就不显示", () => {
    const html = renderToStaticMarkup(<UpdateCard update={controller(sample("ready", { current_version: "0.0.9" }))} />);
    expect(html).toContain("当前 v0.0.9");
    expect(html).not.toContain("最近一次更新");
  });

  it("状态还没取到、也没出错：整块不画", () => {
    expect(renderToStaticMarkup(<UpdateCard update={controller(null)} />)).toBe("");
  });

  it("状态取不到（前后端版本对不上）：说一声，不让这一块悄悄消失", () => {
    const html = renderToStaticMarkup(
      <UpdateCard update={controller(null, { loadError: "更新状态的格式不认识（前后端版本对不上？）" })} />,
    );
    expect(html).toContain("没能读到更新状态");
    expect(html).toContain("前后端版本对不上");
    expect(html).not.toContain("<button");
  });

  it("这台不能更新：只说原因，不给任何会动手的按钮，也不说「等你点立即重启」", () => {
    const tree = UpdateCard({ update: controller(sample("unavailable")) });
    expect(buttons(tree)).toEqual([]);
    const html = renderToStaticMarkup(<UpdateCard update={controller(sample("unavailable"))} />);
    expect(html).toContain("开发版");
    expect(html).not.toContain("等你点");
    expect(renderToStaticMarkup(<UpdateCard update={controller(sample("up_to_date"))} />)).toContain("等你点");
  });

  it("按钮接线：检查更新 → check；立即重启并更新 → restart；开关 → setAutoUpdate(取反)", () => {
    const update = controller(sample("ready"));
    const tree = UpdateCard({ update });
    button(tree, "检查更新").onClick();
    expect(update.check).toHaveBeenCalledTimes(1);
    expect(update.restart).not.toHaveBeenCalled();
    button(tree, "立即重启并更新").onClick();
    expect(update.restart).toHaveBeenCalledTimes(1);
    const toggle = button(tree, "自动检查并下载新版本");
    toggle.onClick();
    expect(update.setAutoUpdate).toHaveBeenCalledWith(false);
    expect(update.check).toHaveBeenCalledTimes(1);
  });

  it("开关：role=switch，名字固定（状态交给 aria-checked，读屏不会念成「…开，开关，开」），可见的开 / 关小药丸只给眼睛看", () => {
    const on = UpdateCard({ update: controller(sample("up_to_date")) });
    const onToggle = button(on, "自动检查并下载新版本");
    expect(onToggle.role).toBe("switch");
    expect(onToggle["aria-checked"]).toBe(true);
    expect(onToggle["aria-label"]).toBe("自动检查并下载新版本");

    const update = controller(sample("up_to_date", { auto_update: false }));
    const off = UpdateCard({ update });
    const offToggle = button(off, "自动检查并下载新版本");
    expect(offToggle["aria-checked"]).toBe(false);
    expect(offToggle["aria-label"]).toBe("自动检查并下载新版本"); // 名字不随状态变
    offToggle.onClick();
    expect(update.setAutoUpdate).toHaveBeenCalledWith(true);

    const onHtml = renderToStaticMarkup(<UpdateCard update={controller(sample("up_to_date"))} />);
    const offHtml = renderToStaticMarkup(<UpdateCard update={update} />);
    expect(onHtml).toMatch(/>开<\/span>/);
    expect(offHtml).toMatch(/>关<\/span>/);
  });

  it("只有「已下载好」才出现「立即重启并更新」；它只被「有命令在途」禁用", () => {
    expect(labels(UpdateCard({ update: controller(sample("up_to_date")) }))).not.toContain("立即重启并更新");
    expect(labels(UpdateCard({ update: controller(sample("ready")) }))).toContain("立即重启并更新");
    expect(button(UpdateCard({ update: controller(sample("ready")) }), "立即重启并更新").disabled).toBe(false);
    expect(button(UpdateCard({ update: controller(sample("ready"), { busy: true }) }), "立即重启并更新").disabled).toBe(true);
  });

  it("「检查更新」：正在检查 / 下载 / 安装、或者有命令在途时禁用（不会并发多次）", () => {
    for (const name of ["checking", "downloading", "installing"]) {
      expect(button(UpdateCard({ update: controller(sample(name)) }), "检查").disabled, name).toBe(true);
    }
    expect(button(UpdateCard({ update: controller(sample("up_to_date"), { busy: true }) }), "检查").disabled).toBe(true);
    expect(button(UpdateCard({ update: controller(sample("up_to_date")) }), "检查更新").disabled).toBe(false);
  });

  it("「检查中…」只在真的在检查时才说：安装中、下载中、点了开关在途，按钮都还是「检查更新」", () => {
    expect(labels(UpdateCard({ update: controller(sample("checking")) }))[0]).toBe("检查中…");
    expect(labels(UpdateCard({ update: controller(sample("installing"), { busy: true }) }))[0]).toBe("检查更新");
    expect(labels(UpdateCard({ update: controller(sample("downloading")) }))[0]).toBe("检查更新");
    expect(labels(UpdateCard({ update: controller(sample("up_to_date"), { busy: true }) }))[0]).toBe("检查更新");
  });

  it("同一句错误只显示一遍：Rust 写进状态的 last_error 和命令返回的错误是同一句（安装失败时就是这样）", () => {
    const status = sample("ready_install_failed");
    const message = status.last_error ?? "";
    const html = renderToStaticMarkup(
      <UpdateCard update={controller(status, { actionError: { source: "restart", message } })} />,
    );
    expect(html.split(message).length - 1).toBe(1);
  });

  it("失败原因（检查失败、命令失败、状态刷新失败）红字全摆出来", () => {
    const html = renderToStaticMarkup(
      <UpdateCard
        update={controller(sample("failed_check"), {
          actionError: { source: "toggle", message: "写不进设置文件" },
          loadError: "更新状态的格式不认识（前后端版本对不上？）",
        })}
      />,
    );
    expect(html).toContain("连不上 GitHub");
    expect(html).toContain("写不进设置文件");
    expect(html).toContain("前后端版本对不上");
  });

  it("安装类失败的标题是「更新没成功」，不许诺自动重试", () => {
    const html = renderToStaticMarkup(<UpdateCard update={controller(sample("failed_install"))} />);
    expect(html).toContain("更新没成功");
    expect(html).not.toContain("自动重试");
  });
});

describe("命令失败的原因", () => {
  it("去掉 Tauri 错误前面的 Error: 前缀", () => {
    expect(cleanError(new Error("没有已经下载好的更新"))).toBe("没有已经下载好的更新");
    expect(cleanError("没有已经下载好的更新")).toBe("没有已经下载好的更新");
  });
});
