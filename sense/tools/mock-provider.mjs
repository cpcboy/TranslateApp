#!/usr/bin/env node
/**
 * 本地 OpenAI 兼容 mock 服务，只用于端到端测试。
 *
 * 它说的是真协议：GET /v1/models、POST /v1/chat/completions（SSE 分片）。
 * 应用走的代码路径与连真实模型时完全一致，区别只是数据是固定的。
 *
 * 按 system 提示词里的标记认请求类型（见 MARKERS）：查词 / 截屏翻译 / 长文翻译 / 语法检查 / 划词解析。
 * 提示词一改、标记对不上，`npm test` 里的 mockProvider.test.ts 会红——
 * 以前截屏提示词改成「你是一位翻译」后这里悄悄认不出了，无 Key 端到端验证对两条链路已经失效却没人发现。
 *
 * 用法：
 *   node tools/mock-provider.mjs            # 监听 11434，对应 Ollama 预设
 *   BAD=1 node tools/mock-provider.mjs      # 故意返回残缺包，测降级
 *   UNTRANSLATED=1 node tools/mock-provider.mjs  # 中文输入仍回中文词条（模拟不听话的模型），测中译英的降级
 *   DELAY=120 PORT=11434 node tools/mock-provider.mjs
 *
 * 每个请求都会追加到 LOG 文件，用来证明「第二次查询 0 请求」。
 */
import { createServer } from "node:http";
import { appendFileSync, realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** 各类请求的 system 提示词里一定出现的一句话；与 Rust 里的提示词逐字对应。 */
export const MARKERS = {
  capture: "你是一位翻译，正在翻译屏幕上一段连续的文字", // capture.rs
  longText: "你是翻译引擎。把 user 给的整篇文本翻译成", // commands/longtext.rs
  grammar: "你是英语语法检查器", // commands/reading.rs
  explain: "你是词典。根据上下文解释", // commands/reading.rs
};

/**
 * 中文输入那一节提示词的标题（system-word-zh.txt 的第一行）。
 * 模型是被提示词教会「先译成英文」的：mock 只有在 system 里看到这一节，才像听话的模型那样回英文词条；
 * 看不到就按老办法回中文词条。这样 Rust 一侧要是忘了给中文输入发这一节，无 Key 的端到端会变红，而不是悄悄「成功」。
 */
export const ZH_SECTION = "# 中文输入";

const CHUNK = 150;

const sense = (rank, pos, gloss_zh, gloss_en, share, register, domains, collocations, text, translation, span, scene, pitfall, synonyms) => ({
  rank, pos, gloss_zh, gloss_en, share, register, domains, collocations,
  examples: [{ text, translation, target_span: span, scene }],
  pitfall, synonyms,
});

const makeEntry = (word, bad, notes = null) => {
  const entry = {
  schema_version: 1,
  word,
  lang: "en",
  pronunciation: { ipa: "/tʃɑːdʒ/", pinyin: null },
  pos: ["v", "n"],
  frequency: { zipf: 5.6, band: "high" },
  senses: [
    sense(1, "v", "收费；要价", "to ask money for goods or a service", 0.42, "neutral", ["消费", "旅行"],
      ["charge sb for sth", "charge a fee"],
      "The hotel charges 200 dollars a night.", "这家酒店每晚收费 200 美元。", "charges", "旅行",
      "人作主语：They charged me ¥30；免费是 free of charge。", ["bill"]),
    sense(2, "v", "指控；控告", "to accuse someone formally of a crime", 0.26, "legal", ["法律", "新闻"],
      ["be charged with sth"],
      "He was charged with fraud last Friday.", "他上周五被控欺诈。", "charged", "法律",
      null, ["accuse"]),
    sense(3, "v", "充电", "to put electricity into a device", 0.16, "neutral", ["技术", "日常"],
      ["charge your phone"],
      "Charge your phone before we leave.", "走之前把手机充上电。", "Charge", "日常",
      null, []),
    sense(4, "n", "掌管；负责", "the position of controlling something", 0.11, "neutral", ["职场"],
      ["in charge of"],
      "She is in charge of the launch.", "她负责这次发布。", "charge", "职场",
      null, ["control"]),
    sense(5, "n", "电量", "the amount of electricity in a battery", 0.05, "technical", ["技术"],
      ["run out of charge"],
      "My laptop ran out of charge mid-meeting.", "开会开到一半笔记本没电了。", "charge", "技术",
      null, []),
  ],
  confusables: [{ word: "cost", difference: "cost 是主语自身花费，charge 是向别人收取。" }],
  notes,
  };
  if (bad) {
    entry.senses = entry.senses.slice(0, 1).map((item) => ({ rank: item.rank, pos: item.pos }));
  }
  return entry;
};

const parseJson = (text, fallback) => {
  try {
    return JSON.parse(text);
  } catch {
    return fallback;
  }
};

/**
 * 中文输入的查词：提示词要求先译成英文、再解析那个英文词。
 * mock 不会翻译，查一张小表；表里没有的，用 charge 那张卡顶（词条不是重点，形状才是）。
 */
const ZH_TO_EN = {
  充电: { word: "recharge", notes: "也可译 charge" },
  落实: { word: "implement", notes: null },
  交代: { word: "explain", notes: "也可译 confess、hand over" },
  微信: { word: "WeChat", notes: null },
};

/** 一个没按提示词来的模型：中文输入仍然给中文词条（以前的行为），用来验证降级路径 */
const chineseEntry = (word, bad) => ({
  ...makeEntry(word, bad),
  lang: "zh",
  pronunciation: { ipa: null, pinyin: "zhōng wén" },
});

/** 按请求类型算出整段要回的文本 */
function answerFor(body, bad, untranslated = false) {
  const system = body.messages?.[0]?.content ?? "";
  const user = body.messages?.[1]?.content ?? "";

  if (system.includes(MARKERS.capture)) {
    // 截屏翻译：user 是字符串数组，回等长的译文数组
    const items = parseJson(user, []);
    return JSON.stringify(items.map((text, index) => `译文${index + 1} ${String(text).slice(0, 6)}`));
  }
  if (system.includes(MARKERS.longText)) {
    // 长文翻译：user 就是整篇原文，保留分段，回纯文本
    return String(user)
      .split("\n")
      .map((line) => (line.trim() ? `【译】${line}` : line))
      .join("\n");
  }
  if (system.includes(MARKERS.grammar)) {
    return JSON.stringify(
      String(user).includes("He go ")
        ? [{ fragment: "He go", replacement: "He goes", explanation: "主语是第三人称单数，动词要加 -es" }]
        : [],
    );
  }
  if (system.includes(MARKERS.explain)) {
    const { term = "" } = parseJson(user, {});
    return JSON.stringify({ gloss: `（mock）「${term}」在这句话里的意思`, pos: "n.", note: "mock 数据，不是真实讲解" });
  }
  // 其余都当查词：把请求的词填进卡片
  const { word = "charge", lang = "en" } = parseJson(user, {});
  if (lang === "zh") {
    if (untranslated || !system.includes(ZH_SECTION)) return JSON.stringify(chineseEntry(word, bad));
    const english = ZH_TO_EN[word] ?? { word: "charge", notes: null };
    return JSON.stringify(makeEntry(english.word, bad, english.notes));
  }
  return JSON.stringify(makeEntry(word, bad));
}

/** 造一个 mock 服务（还没开始监听）。 */
export function createMock({ delay = 35, bad = false, untranslated = false, log: logFile = "/tmp/sense-mock-requests.log" } = {}) {
  const log = (line) => {
    if (!logFile) return;
    try {
      appendFileSync(logFile, `${new Date().toISOString()} ${line}\n`);
    } catch {
      /* 日志写不了不影响服务 */
    }
  };

  return createServer((req, res) => {
  log(`${req.method} ${req.url}`);

  if (req.method === "GET" && req.url?.endsWith("/models")) {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ object: "list", data: [{ id: "mock-word", object: "model" }] }));
    return;
  }

  if (req.method === "POST" && req.url?.endsWith("/chat/completions")) {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.resume();
    req.on("end", () => {
      // 把请求体的关键开关记下来：验证「关思考」到底有没有发出去
      let source = "";
      const body = parseJson(Buffer.concat(chunks).toString("utf8"), null);
      if (body) {
        const thinking =
          body.thinking ?? body.enable_thinking ?? body.reasoning ?? body.reasoning_effort ?? null;
        log(
          `  > model=${body.model} max_tokens=${body.max_tokens ?? "-"} temperature=${body.temperature ?? "-"} thinking=${JSON.stringify(thinking)}`,
        );
        source = answerFor(body, bad, untranslated);
      } else {
        log("  > body 解析失败");
        source = answerFor({}, bad, untranslated);
      }
      res.writeHead(200, {
        "Content-Type": "text/event-stream",
        "Cache-Control": "no-cache",
        Connection: "keep-alive",
      });

      let cursor = 0;
      const timer = setInterval(() => {
        if (cursor >= source.length) {
          clearInterval(timer);
          res.write("data: [DONE]\n\n");
          res.end();
          return;
        }
        const piece = source.slice(cursor, cursor + CHUNK);
        cursor += CHUNK;
        res.write(`data: ${JSON.stringify({ choices: [{ delta: { content: piece } }] })}\n\n`);
      }, delay);

      // 只在客户端断开（res 关闭）时收尾；req 的 close 会在请求体读完就触发。
      res.on("close", () => clearInterval(timer));
    });
    return;
  }

  res.writeHead(404, { "Content-Type": "application/json" });
  res.end(JSON.stringify({ error: { message: "not found" } }));
});
}

// 直接 `node tools/mock-provider.mjs` 运行时才监听；被测试 import 时只导出工厂
// 两边都取真实路径：经符号链接启动时 argv[1] 和 import.meta.url 的写法不同，直接比会静默不监听
const isMain = (() => {
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();
if (isMain) {
  const port = Number(process.env.PORT ?? 11434);
  const delay = Number(process.env.DELAY ?? 35);
  const bad = process.env.BAD === "1";
  const untranslated = process.env.UNTRANSLATED === "1";
  createMock({ delay, bad, untranslated, log: process.env.LOG ?? "/tmp/sense-mock-requests.log" }).listen(port, "127.0.0.1", () => {
    console.log(`[mock] listening on http://127.0.0.1:${port}/v1  (bad=${bad}, untranslated=${untranslated}, delay=${delay}ms)`);
  });
}
