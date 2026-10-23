#!/usr/bin/env node
/**
 * 本地 OpenAI 兼容 mock 服务，只用于端到端测试。
 *
 * 它说的是真协议：GET /v1/models、POST /v1/chat/completions（SSE 分片）。
 * 应用走的代码路径与连真实模型时完全一致，区别只是数据是固定的。
 *
 * 用法：
 *   node tools/mock-provider.mjs            # 监听 11434，对应 Ollama 预设
 *   BAD=1 node tools/mock-provider.mjs      # 故意返回残缺包，测降级
 *   DELAY=120 PORT=11434 node tools/mock-provider.mjs
 *
 * 每个请求都会追加到 LOG 文件，用来证明「第二次查询 0 请求」。
 */
import { createServer } from "node:http";
import { appendFileSync } from "node:fs";

const PORT = Number(process.env.PORT ?? 11434);
const DELAY = Number(process.env.DELAY ?? 35);
const CHUNK = 150;
const LOG = process.env.LOG ?? "/tmp/sense-mock-requests.log";
const BAD = process.env.BAD === "1";

const sense = (rank, pos, gloss_zh, gloss_en, share, register, domains, collocations, text, translation, span, scene, pitfall, synonyms) => ({
  rank, pos, gloss_zh, gloss_en, share, register, domains, collocations,
  examples: [{ text, translation, target_span: span, scene }],
  pitfall, synonyms,
});

const entry = {
  schema_version: 1,
  word: "charge",
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
  notes: null,
};

if (BAD) {
  entry.senses = entry.senses.slice(0, 1).map((item) => ({ rank: item.rank, pos: item.pos }));
}

const payload = JSON.stringify(entry);

function log(line) {
  try {
    appendFileSync(LOG, `${new Date().toISOString()} ${line}\n`);
  } catch {
    /* 日志写不了不影响服务 */
  }
}

const server = createServer((req, res) => {
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
      try {
        const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
        const thinking =
          body.thinking ?? body.enable_thinking ?? body.reasoning ?? body.reasoning_effort ?? null;
        log(
          `  > model=${body.model} max_tokens=${body.max_tokens ?? "-"} temperature=${body.temperature ?? "-"} thinking=${JSON.stringify(thinking)}`,
        );
      } catch {
        log("  > body 解析失败");
      }
      res.writeHead(200, {
        "Content-Type": "text/event-stream",
        "Cache-Control": "no-cache",
        Connection: "keep-alive",
      });

      let cursor = 0;
      const timer = setInterval(() => {
        if (cursor >= payload.length) {
          clearInterval(timer);
          res.write("data: [DONE]\n\n");
          res.end();
          return;
        }
        const piece = payload.slice(cursor, cursor + CHUNK);
        cursor += CHUNK;
        res.write(`data: ${JSON.stringify({ choices: [{ delta: { content: piece } }] })}\n\n`);
      }, DELAY);

      // 只在客户端断开（res 关闭）时收尾；req 的 close 会在请求体读完就触发。
      res.on("close", () => clearInterval(timer));
    });
    return;
  }

  res.writeHead(404, { "Content-Type": "application/json" });
  res.end(JSON.stringify({ error: { message: "not found" } }));
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`[mock] listening on http://127.0.0.1:${PORT}/v1  (bad=${BAD}, delay=${DELAY}ms)`);
});
