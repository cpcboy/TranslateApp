import { afterEach, describe, expect, it, vi } from "vitest";
import fixture from "../../lib/updateStatus.fixture.json";
import { parseUpdateStatus, type UpdateStatus } from "../../lib/updateStatus";
import {
  actionErrorExpired,
  alertToLatch,
  bannerKey,
  bannerModel,
  canCheck,
  describeStatus,
  formatBytes,
  formatTime,
  isBannerDismissed,
  progressPercent,
  shouldAccept,
  showBanner,
  showLastUpdate,
  visibleErrors,
} from "./describe";

const sample = (name: string, over: Partial<UpdateStatus> = {}): UpdateStatus => {
  const status = parseUpdateStatus((fixture as Record<string, unknown>)[name]);
  if (!status) throw new Error(`夹具 ${name} 解析不了`);
  return { ...status, ...over };
};

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("字节数", () => {
  it("按量级换算，别出现 NaN / 负数", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2 KB");
    expect(formatBytes(2097152)).toBe("2.0 MB");
    expect(formatBytes(9437184)).toBe("9.0 MB");
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe("—");
  });

  it("换档的边界：四舍五入之后够 1024 KB 就算 MB，不显示「1024 KB」", () => {
    expect(formatBytes(1023)).toBe("1023 B");
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(1048575)).toBe("1.0 MB");
    expect(formatBytes(1048576)).toBe("1.0 MB");
    expect(formatBytes(1047552)).toBe("1023 KB");
  });
});

describe("下载百分比", () => {
  it("取整数，夹在 0–100 之间；不知道总长度就是 null（不是 0%）", () => {
    expect(progressPercent(0, 100)).toBe(0);
    expect(progressPercent(50, 100)).toBe(50);
    expect(progressPercent(2097152, 9437184)).toBe(22);
    expect(progressPercent(100, 100)).toBe(100);
    expect(progressPercent(150, 100)).toBe(100);
    expect(progressPercent(-5, 100)).toBe(0);
    expect(progressPercent(10, null)).toBeNull();
    expect(progressPercent(10, 0)).toBeNull();
  });
});

describe("时间", () => {
  it("只显示月日时分，补零，用本地时区", () => {
    expect(formatTime(new Date(2026, 10, 1, 14, 5).getTime())).toBe("11-01 14:05");
    expect(formatTime(new Date(2026, 0, 9, 0, 0).getTime())).toBe("01-09 00:00");
    expect(formatTime(Number.NaN)).toBe("—");
  });

  it("同一个时刻在不同时区显示不同（确实是本地时间，不是 UTC）", () => {
    const instant = Date.UTC(2026, 10, 1, 6, 30); // 2026-11-01 06:30 UTC
    vi.stubEnv("TZ", "Asia/Shanghai");
    expect(formatTime(instant)).toBe("11-01 14:30");
    vi.stubEnv("TZ", "America/Los_Angeles");
    expect(formatTime(instant)).toBe("10-31 23:30");
    vi.stubEnv("TZ", "UTC");
    expect(formatTime(instant)).toBe("11-01 06:30");
  });
});

describe("能不能点「检查更新」", () => {
  it("空闲、已是最新、已就绪时可以；正在检查 / 下载 / 安装时不行；这台不能更新时不行", () => {
    expect(canCheck(sample("idle"))).toBe(true);
    expect(canCheck(sample("up_to_date"))).toBe(true);
    expect(canCheck(sample("ready"))).toBe(true);
    expect(canCheck(sample("failed_check"))).toBe(true);
    expect(canCheck(sample("failed_install"))).toBe(true);
    expect(canCheck(sample("checking"))).toBe(false);
    expect(canCheck(sample("downloading"))).toBe(false);
    expect(canCheck(sample("installing"))).toBe(false);
    expect(canCheck(sample("unavailable"))).toBe(false);
  });
});

describe("顶部横幅画什么", () => {
  it("已下载好：ready，带版本；安装中：installing，不带错误", () => {
    expect(bannerModel(sample("ready"))).toEqual({ kind: "ready", version: "0.1.1", error: null });
    expect(bannerModel(sample("installing"))).toEqual({ kind: "installing", version: "0.1.1", error: null });
  });

  it("已下载好、但上一次安装没成（取消了授权，包还在）：仍是 ready，错误跟着带出来", () => {
    const model = bannerModel(sample("ready_install_failed"));
    expect(model?.kind).toBe("ready");
    expect(model?.version).toBe("0.1.1");
    expect(model?.error).toContain("管理员授权");
  });

  it("安装 / 重启没成而且包已经不能再试：failed，错误在主界面就看得见（只放设置页等于没说）", () => {
    const model = bannerModel(sample("failed_install"));
    expect(model?.kind).toBe("failed");
    expect(model?.error).toContain("旧版本没能放回原位");
  });

  it("检查类失败不上横幅（到点会自动重试，设置页里说得清楚）；其余阶段、状态没取到、这台不能更新，都不出现", () => {
    for (const name of ["idle", "checking", "up_to_date", "downloading", "downloading_unknown_size", "failed_check", "unavailable"]) {
      expect(bannerModel(sample(name)), name).toBeNull();
      expect(showBanner(sample(name)), name).toBe(false);
    }
    expect(bannerModel(null)).toBeNull();
    expect(showBanner(null)).toBe(false);
    // 即便阶段是 ready / 有安装错误，不能更新的环境也不该催用户重启
    expect(bannerModel({ ...sample("ready"), unavailable: "开发版" })).toBeNull();
    expect(bannerModel({ ...sample("failed_install"), unavailable: "开发版" })).toBeNull();
  });

  it("安装失败之后又在复查：错误还挂着，横幅不能因为阶段变成「检查中」就收走", () => {
    expect(bannerModel({ ...sample("failed_install"), phase: { state: "checking" } })?.kind).toBe("failed");
  });

  it("正在下载别的版本时，状态里残留的安装类错误不画「失败」（多半是可重试的失败又赶上复查发现了更新的版本，说「需要你处理」是错的）", () => {
    const status = sample("downloading", { last_error: "没能替换「应用程序」里的 SENSE", error_kind: "install" });
    expect(bannerModel(status)).toBeNull();
  });

  it("锁存的失败（alert）：状态里的错误被后台检查清掉 / 覆盖之后横幅还在；下载别的版本时也在；已就绪时带在「立即重启」那条里；安装中不带", () => {
    const message = "安装中途失败，而且旧版本没能放回原位。请手动把它拖回「应用程序」文件夹";
    expect(bannerModel(sample("up_to_date"), message)).toEqual({ kind: "failed", version: "", error: message });
    expect(bannerModel(sample("idle"), message)?.kind).toBe("failed");
    expect(bannerModel(sample("failed_check"), message)?.error).toBe(message); // 检查失败覆盖了 last_error 也一样
    expect(bannerModel(sample("downloading"), message)?.kind).toBe("failed");
    expect(bannerModel(sample("ready"), message)).toEqual({ kind: "ready", version: "0.1.1", error: message });
    expect(bannerModel(sample("installing"), message)).toEqual({ kind: "installing", version: "0.1.1", error: null });
    expect(showBanner(sample("up_to_date"), message)).toBe(true);
    // 状态里自己带着的错误优先
    expect(bannerModel(sample("failed_install"), "别的话")?.error).toContain("旧版本没能放回原位");
    // 这台不能更新 / 状态没取到：什么都不画
    expect(bannerModel({ ...sample("up_to_date"), unavailable: "开发版" }, message)).toBeNull();
    expect(bannerModel(null, message)).toBeNull();
    // 没有锁存、状态也没错：没有横幅
    expect(bannerModel(sample("up_to_date"), null)).toBeNull();
  });

  it("showBanner 与 bannerModel 一致", () => {
    for (const name of Object.keys(fixture)) {
      expect(showBanner(sample(name)), name).toBe(bannerModel(sample(name)) !== null);
    }
  });
});

describe("什么时候把失败锁存起来", () => {
  it("只有「从正在安装直接落到失败、而且没有包可重试」才锁", () => {
    expect(alertToLatch(true, sample("failed_install"))).toContain("旧版本没能放回原位");
  });

  it("回到已就绪（可重试的失败：取消了授权，包还在）不锁——横幅里本来就有「立即重启」", () => {
    expect(alertToLatch(true, sample("ready_install_failed"))).toBeNull();
  });

  it("之前不是在安装：不锁（下载别的版本、复查、重新取到的初始状态都不会误锁）", () => {
    expect(alertToLatch(false, sample("failed_install"))).toBeNull();
  });

  it("安装中本身、检查类失败、成功、没有错误文字：不锁", () => {
    expect(alertToLatch(true, sample("installing"))).toBeNull();
    expect(alertToLatch(true, sample("failed_check"))).toBeNull();
    expect(alertToLatch(true, sample("up_to_date"))).toBeNull();
    expect(alertToLatch(true, { ...sample("failed_install"), last_error: null })).toBeNull();
    expect(alertToLatch(true, { ...sample("failed_install"), last_error: "" })).toBeNull();
  });
});

describe("点过「稍后」的横幅要不要收起来", () => {
  const ready = bannerModel(sample("ready"))!;
  const readyFailed = bannerModel(sample("ready_install_failed"))!;
  const installing = bannerModel(sample("installing"))!;
  const failed = bannerModel(sample("failed_install"))!;

  it("收起点过的那一条；没点过照常显示", () => {
    expect(isBannerDismissed(bannerKey(ready), ready)).toBe(true);
    expect(isBannerDismissed(null, ready)).toBe(false);
  });

  it("出了更新的版本、或者同一版出了新的错误，都是另一条，会再出现", () => {
    const newer = bannerModel({ ...sample("ready"), phase: { state: "ready", version: "0.1.2" } })!;
    expect(isBannerDismissed(bannerKey(ready), newer)).toBe(false);
    // 点「稍后」时没有错误，之后点「立即重启」装失败了：必须让人看见
    expect(isBannerDismissed(bannerKey(ready), readyFailed)).toBe(false);
    // 反过来也是：错误变了
    const otherError = { ...readyFailed, error: "另一种失败" };
    expect(isBannerDismissed(bannerKey(readyFailed), otherError)).toBe(false);
  });

  it("有错误时「稍后」照样能收起（不能因为带着错误就成了点不动的按钮）", () => {
    expect(isBannerDismissed(bannerKey(readyFailed), readyFailed)).toBe(true);
    expect(isBannerDismissed(bannerKey(failed), failed)).toBe(true);
  });

  it("正在安装的永远不收起（用户刚点的，要看到进度）", () => {
    expect(isBannerDismissed(bannerKey(installing), installing)).toBe(false);
    expect(isBannerDismissed("0.1.1|", installing)).toBe(false);
  });
});

describe("每个阶段怎么说", () => {
  it("没检查过：说明什么时候会自动检查；自动检查关了就明说", () => {
    const on = describeStatus(sample("idle"));
    expect(on.headline).toBe("还没检查过");
    expect(on.detail).toContain("半分钟");
    expect(on.tone).toBe("muted");
    const off = describeStatus({ ...sample("idle"), auto_update: false });
    expect(off.detail).toContain("已关闭");
  });

  it("检查失败要如实说失败并带出原因，不能冒充「已是最新」", () => {
    const view = describeStatus(sample("failed_check"));
    expect(view.headline).toBe("更新没成功");
    expect(view.headline).not.toContain("最新");
    expect(view.error).toContain("GitHub");
    expect(view.tone).toBe("error");
  });

  it("「之后会自动重试」只在真的会重试时才说：检查类失败 + 自动检查开着", () => {
    expect(describeStatus(sample("failed_check")).detail).toMatch(/之后会自动重试$/);
    // 自动检查关着：不会重试，说清楚怎么办
    const off = describeStatus(sample("failed_check", { auto_update: false }));
    expect(off.detail).not.toContain("会自动重试");
    expect(off.detail).toContain("自动检查已关闭");
    expect(off.detail).toContain("点「检查更新」");
    // 安装类失败：从来不会自动重试，错误本身写着该怎么办
    const install = describeStatus(sample("failed_install"));
    expect(install.detail).not.toContain("重试");
    expect(install.error).toContain("旧版本没能放回原位");
    expect(install.headline).toBe("更新没成功");
  });

  it("没有检查时间的失败也不出错，不留多余的逗号", () => {
    const view = describeStatus(sample("failed_check", { checked_at: null }));
    expect(view.detail).toBe("之后会自动重试");
  });

  it("已是最新：带上次检查时间", () => {
    const view = describeStatus(sample("up_to_date"));
    expect(view.headline).toBe("已是最新版本");
    expect(view.detail).toMatch(/^上次检查 \d{2}-\d{2} \d{2}:\d{2}$/);
    expect(view.tone).toBe("ok");
  });

  it("下载中：有总长度显示百分比与大小，没有就只显示已下载的", () => {
    const known = describeStatus(sample("downloading"));
    expect(known.headline).toBe("正在下载 v0.1.1");
    expect(known.detail).toBe("22% · 2.0 MB / 9.0 MB");
    expect(known.tone).toBe("busy");
    const unknown = describeStatus(sample("downloading_unknown_size"));
    expect(unknown.detail).toBe("已下载 512 B");
  });

  it("已就绪：说清已通过签名校验、点哪里完成、自己退出再打开不会更新；有更新说明就带上更新说明", () => {
    const withNotes = describeStatus(sample("ready"));
    expect(withNotes.headline).toBe("v0.1.1 已下载并通过签名校验");
    expect(withNotes.detail).toContain("修了几个 bug");
    expect(withNotes.detail).toContain("立即重启");
    expect(withNotes.detail).toContain("自己退出再打开不会更新");
    const without = describeStatus({ ...sample("ready"), notes: null });
    expect(without.detail).toContain("立即重启");
    expect(without.detail).toContain("自己退出再打开不会更新");
    expect(without.tone).toBe("ok");
  });

  it("已就绪之后复查 / 安装失败：仍是已就绪，同时把失败原因亮出来", () => {
    const view = describeStatus(sample("ready_install_failed"));
    expect(view.headline).toContain("已下载");
    expect(view.error).toContain("管理员授权");
  });

  it("安装中、检查中、下载中：忙的时候不挂着上一次的失败（重试时红字还在，像是「这次又失败了」）", () => {
    expect(describeStatus(sample("installing")).headline).toBe("正在安装 v0.1.1…");
    expect(describeStatus(sample("checking")).headline).toBe("正在检查更新…");
    const stale = "连不上 GitHub";
    for (const name of ["checking", "downloading", "installing"]) {
      expect(describeStatus(sample(name, { last_error: stale, error_kind: "check" })).error, name).toBeNull();
    }
  });

  it("这台不能更新：原因原样给出，不展示别的状态", () => {
    const view = describeStatus(sample("unavailable"));
    expect(view.headline).toBe("这里不能自动更新");
    expect(view.detail).toContain("开发版");
    expect(view.error).toBeNull();
  });

  it("所有夹具都能说出话来：标题非空，只有出错时才有 error", () => {
    for (const name of Object.keys(fixture)) {
      const status = sample(name);
      const view = describeStatus(status);
      expect(view.headline.length, name).toBeGreaterThan(0);
      if (view.error !== null) expect(status.last_error, name).toBe(view.error);
    }
  });
});

describe("最近一次更新什么时候显示", () => {
  it("只有它确实是当前这一版才显示（手动降级装了旧版之后，写着「→ 0.1.0」而当前是 0.0.9 就是误导）", () => {
    expect(showLastUpdate(sample("ready"))).toBe(true); // 夹具：to 0.1.0 = 当前 0.1.0
    expect(showLastUpdate(sample("ready", { current_version: "0.0.9" }))).toBe(false);
    expect(showLastUpdate(sample("up_to_date"))).toBe(false); // 从没更新过
  });
});

describe("乱序的状态怎么取舍", () => {
  it("序号比已采纳的最大序号旧的丢掉，相同或更新的采纳", () => {
    expect(shouldAccept(5, 4)).toBe(false);
    expect(shouldAccept(5, 5)).toBe(true);
    expect(shouldAccept(5, 6)).toBe(true);
    expect(shouldAccept(-1, 0)).toBe(true);
  });
});

describe("命令失败的原因什么时候撤掉", () => {
  it("失败之后又来了更新的状态才撤；失败那一刻发出的状态（序号不比记下的大）不撤", () => {
    expect(actionErrorExpired(7, 7)).toBe(false);
    expect(actionErrorExpired(7, 6)).toBe(false);
    expect(actionErrorExpired(7, 8)).toBe(true);
    // 没有失败过时记的是 -1：任何状态都「更新」，撤一个本来就没有的错误是空操作
    expect(actionErrorExpired(-1, 0)).toBe(true);
  });
});

describe("红字错误显示哪几行", () => {
  it("同一句话只显示一遍（Rust 写进状态的 last_error 与命令返回的错误常常是同一句）；空的不显示；保持先后顺序", () => {
    expect(visibleErrors("没有权限", "没有权限")).toEqual(["没有权限"]);
    expect(visibleErrors("没有权限", "写不进设置")).toEqual(["没有权限", "写不进设置"]);
    expect(visibleErrors(null, "写不进设置")).toEqual(["写不进设置"]);
    expect(visibleErrors("没有权限", null)).toEqual(["没有权限"]);
    expect(visibleErrors(null, null)).toEqual([]);
    expect(visibleErrors("", "")).toEqual([]);
  });
});
