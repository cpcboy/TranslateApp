import { describe, expect, it } from "vitest";
// 用 Vite 的 ?raw 读源码：不引入 DOM 测试库，也不装依赖
import appSource from "../../App.tsx?raw";
import longTextViewSource from "./LongTextView.tsx?raw";
import lookupViewSource from "./LookupView.tsx?raw";

/**
 * 接线钉子。
 *
 * LookupView / LongTextView 在 node 下渲染不出交互（没有 DOM，effect 不执行），逻辑已经抽成纯函数单测了
 * （`inputFlow.ts`、`reading.ts`、`languages.ts`、`store.ts`），但「组件有没有把它们接上」没人盯着：
 * 评审员往这些接线里打了 31 个变异，17 个存活（重试改回绕过路由、Esc 监听挂在冒泡阶段、打开选择器不清旧错误、语法检查写死英文……）。
 *
 * 这里只钉最容易悄悄坏掉的几处，写法有意宽松（折叠空白后按片段匹配）。它们是**接线**钉子，不是行为测试：
 * 行为由真实二进制和浏览器里的实测验证过（见 CHANGELOG）。改了接线、这里变红时，先确认新写法是对的，再改这里的片段。
 */
const flat = (source: string) => source.replace(/\s+/g, " ");
const lookupView = flat(lookupViewSource);
const longTextView = flat(longTextViewSource);
const app = flat(appSource);

describe("LookupView 的接线", () => {
  it("选择器的 Esc：用 pickerEscHandler，挂在 window 的**捕获阶段**（第三个参数 true），摘的时候也是；没展开时不挂", () => {
    expect(lookupView).toContain("pickerEscHandler({");
    expect(lookupView).toContain('window.addEventListener("keydown", onKey, true)');
    expect(lookupView).toContain('window.removeEventListener("keydown", onKey, true)');
    expect(lookupView).toContain("if (!pickerOpen) return;");
  });

  it("输入框提交走 submitInput（先收选择器、再问路由）", () => {
    expect(lookupView).toContain("submitInput(raw, inputActions)");
    // 回车与示例按钮都走 submit：示例按钮不绕路由
    expect(lookupView).toContain("void submit();");
    expect(lookupView).toContain("void submit(word);");
  });

  it("重试也走路由（失败之后换了目标语言再点要重新分流），但不碰输入框里用户后来打的草稿", () => {
    expect(lookupView).toContain("submitInput(value, { ...inputActions, clearInput: () => {} })");
    expect(lookupView).toContain("onRetry={() => void retry(query)}");
    expect(lookupView).not.toMatch(/onRetry=\{\(\) => void run\(/);
  });

  it("打开选择器时清掉上一回的失败提示", () => {
    expect(lookupView).toContain("if (!pickerOpen) clearError();");
  });

  it("选语言：存成功才收起（pickTarget），面板的 onChoose 接的就是它", () => {
    expect(lookupView).toContain("onChoose={(code) => void pickTarget(code, choose, { closePicker, focusInput })}");
  });

  it("阅读视图拿到两端语言", () => {
    expect(lookupView).toContain("languages={longLang}");
  });

  it("提交动作表：run 接 run、runLong 接 runLong（对调了，句子进查词卡、单词进阅读视图），提交时清输入框、收选择器", () => {
    expect(lookupView).toContain("run: (value: string) => void run(value),");
    expect(lookupView).toContain("runLong: (value: string) => void runLong(value),");
    expect(lookupView).toContain('clearInput: () => setText(""),');
    expect(lookupView).toMatch(/const inputActions = \{ routeInput,[^}]*\bclosePicker,\s*\};/);
  });

  it("回车：输入法组字时不拦（isComposing 与 WebKit 的 keyCode 229 都要查），Shift+Enter 换行", () => {
    expect(lookupView).toContain(
      'event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229',
    );
  });

  it("选择器按钮能开也能关", () => {
    expect(lookupView).toContain("setPickerOpen(!pickerOpen);");
  });
});

describe("LongTextView 的接线", () => {
  it("语法检查的语种来自 Rust 确认的原文语言，不写死", () => {
    expect(longTextView).toContain("const sourceLang = confirmedSourceCode(languages);");
    expect(longTextView).toMatch(/shouldCheckGrammar\(\{[^}]*\bsourceLang\b[^}]*\}\)/);
    expect(longTextView).not.toMatch(/sourceLang\s*[:=]\s*["']en["']/);
  });

  it("两栏标题标语种", () => {
    expect(longTextView).toContain("const titles = paneTitles(languages);");
    expect(longTextView).toContain("title={titles.source}");
    expect(longTextView).toContain("title={titles.target}");
  });

  it("译文和原文一样：写完、没出错、没中断才说", () => {
    expect(longTextView).toContain("!busy && !error && !incomplete && text && isEcho(source, text)");
    expect(longTextView).toContain("<EchoNotice");
  });

  it("译文和原文一样的说明知道原文语言是否已确认（确认了就不说「原文可能本来就是目标语言」）", () => {
    expect(longTextView).toContain("sourceKnown={sourceLang !== null && sourceLang !== languages?.target.code}");
  });
});

describe("App", () => {
  it("debug 构建的 SENSE_DEV_LOOKUP 走和回车同一条路由", () => {
    expect(app).toContain("const route = await routeInput(input);");
    expect(app).toMatch(/route === "long"\) void runLong\(input\);\s*else if \(route === "word"\) void run\(input\);/);
  });
});
