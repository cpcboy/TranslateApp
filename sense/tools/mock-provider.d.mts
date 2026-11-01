/** 给 TypeScript 看的类型声明：mockProvider.test.ts 要 import 这个 mock。 */
export const MARKERS: {
  capture: string;
  longText: string;
  grammar: string;
  explain: string;
};

/** 认出请求类型和翻译目标语言（长文、截屏提示词里的「翻译成{语言名}」）；只用于日志 */
export function describeRequest(body: { messages?: Array<{ content?: string }> } | null | undefined): {
  task: "capture" | "longText" | "grammar" | "explain" | "word";
  target: string | null;
};

/** 中文输入那一节提示词的标题；system 里有它，mock 才会回译成英文的词条 */
export const ZH_SECTION: string;

export type MockServer = {
  listen(port: number, host: string, callback?: () => void): unknown;
  close(callback?: () => void): unknown;
  address(): { port: number } | string | null;
};

export function createMock(options?: {
  delay?: number;
  bad?: boolean;
  /** 中文输入仍回中文词条，模拟没按提示词来的模型 */
  untranslated?: boolean;
  log?: string | null;
}): MockServer;
