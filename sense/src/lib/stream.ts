/**
 * 增量 JSON 修复解析（方案 §8.3）。
 *
 * 模型是边生成边吐的，缓冲区里永远是半截 JSON。这里把它补成合法 JSON，
 * 再把最后那个可能没写完的义项丢掉——已完整的义项先渲染出来。
 */

/** 去掉 ```json 围栏和模型偶尔加的前后缀，只留 JSON 主体。 */
export function extractJson(raw: string): string {
  let text = raw.trim();

  const fence = text.match(/```(?:json)?\s*([\s\S]*?)```/i);
  if (fence?.[1]) text = fence[1].trim();

  const start = text.indexOf("{");
  if (start > 0) text = text.slice(start);

  const end = text.lastIndexOf("}");
  if (end >= 0 && end < text.length - 1) text = text.slice(0, end + 1);

  return text;
}

/**
 * 把没闭合的字符串、数组、对象补齐。
 *
 * 注意：不能像某些示例那样「先补 ] 再补 }」——嵌套结构里顺序反了就是非法 JSON。
 * 这里按括号栈逆序闭合；如果断在半截 key 上，就把这个成员整个丢掉。
 */
export function closeOpen(input: string): string {
  const trimmed = input.replace(/,\s*$/, "");
  const closed = balance(trimmed);
  if (parses(closed)) return closed;

  // 断在半截 key 上：丢掉最后一个成员再试一次
  const dropped = dropLastMember(trimmed);
  if (dropped !== null) {
    const retried = balance(dropped);
    if (parses(retried)) return retried;
  }
  return closed;
}

function parses(text: string): boolean {
  try {
    JSON.parse(text);
    return true;
  } catch {
    return false;
  }
}

function balance(input: string): string {
  let inString = false;
  let escaped = false;
  let quoteStart = -1;
  const stack: string[] = [];

  for (let index = 0; index < input.length; index += 1) {
    const ch = input[index];
    if (inString) {
      if (escaped) escaped = false;
      else if (ch === "\\") escaped = true;
      else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"') {
      inString = true;
      quoteStart = index;
      continue;
    }
    if (ch === "{" || ch === "[") stack.push(ch);
    else if (ch === "}" || ch === "]") stack.pop();
  }

  let out = input;

  if (inString) {
    // 这个未闭合的字符串是「键」还是「值」？前面是 , 或 { 就说明是键。
    const before = input.slice(0, quoteStart).trimEnd();
    const isKey = before.endsWith(",") || before.endsWith("{");
    if (isKey) {
      const withoutMember = before.endsWith(",") ? before.slice(0, -1) : before;
      return balance(withoutMember);
    }
    out += '"';
  }

  for (let index = stack.length - 1; index >= 0; index -= 1) {
    out += stack[index] === "{" ? "}" : "]";
  }
  return out;
}

/** 丢掉最后一个「键值对 / 数组成员」，用于半截 key 的兜底。 */
function dropLastMember(input: string): string | null {
  for (let index = input.length - 1; index >= 0; index -= 1) {
    const ch = input[index];
    if (ch === ",") {
      return input.slice(0, index);
    }
    if (ch === "{" || ch === "[") {
      return input.slice(0, index + 1);
    }
  }
  return null;
}

function isCompleteSense(sense: unknown): boolean {
  if (!sense || typeof sense !== "object") return false;
  const item = sense as Record<string, unknown>;
  if (typeof item.gloss_zh !== "string" || item.gloss_zh.length === 0) return false;
  if (!Array.isArray(item.examples) || item.examples.length === 0) return false;
  const example = item.examples[0] as Record<string, unknown> | undefined;
  return Boolean(example && typeof example.text === "string" && example.text.length > 0);
}

export type PartialEntry = {
  word?: string;
  lang?: string;
  pronunciation?: { ipa?: string | null; pinyin?: string | null };
  frequency?: { zipf?: number | null; band?: string | null };
  senses: unknown[];
  [key: string]: unknown;
};

/**
 * 尝试从半截缓冲区里解析出「已经完整的义项」。
 *
 * 解析不出来返回 null（调用方继续等下一片），不要抛异常——
 * 流式过程里异常一次就会打断整张卡。
 */
export function tryParsePartial(buffer: string): PartialEntry | null {
  const text = extractJson(buffer);
  if (!text) return null;

  try {
    const parsed = JSON.parse(text) as PartialEntry;
    if (Array.isArray(parsed?.senses) && parsed.senses.length > 0) return parsed;
  } catch {
    /* 落到下面的补全逻辑 */
  }

  let parsed: PartialEntry;
  try {
    parsed = JSON.parse(closeOpen(text)) as PartialEntry;
  } catch {
    return null;
  }

  if (!parsed || !Array.isArray(parsed.senses)) return null;

  // 只保留结构完整的义项：最后一条常常是写了一半的。
  const complete = parsed.senses.filter(isCompleteSense);
  if (complete.length === 0) return null;

  return { ...parsed, senses: complete };
}

export type FinalParse = { value: unknown; repaired: boolean };

/**
 * 最终结果。`repaired` 表示这份 JSON 是靠补括号救回来的——
 * 也就是「模型被截断了」，调用方必须把它当降级处理，不能假装完整。
 */
export function parseFinal(buffer: string): FinalParse | null {
  const text = extractJson(buffer);
  try {
    return { value: JSON.parse(text), repaired: false };
  } catch {
    try {
      return { value: JSON.parse(closeOpen(text)), repaired: true };
    } catch {
      return null;
    }
  }
}
