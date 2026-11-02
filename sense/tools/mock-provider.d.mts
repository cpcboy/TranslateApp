/** 给 TypeScript 看的类型声明：mockProvider.test.ts 要 import 这个 mock。 */
export const MARKERS: {
  capture: string;
  longText: string;
  grammar: string;
  explain: string;
};

/** 中文输入那一节提示词的标题；system 里有它，mock 才会回译成英文的词条 */
export const ZH_SECTION: string;

export type MockServer = {
  listen(port: number, host: string, callback?: () => void): unknown;
  close(callback?: () => void): unknown;
  address(): { port: number } | string | null;
};

/** `DROP=3,8` → [3, 8]；空 / 没设 / 不是整数的都丢掉 */
export function parseDrop(value: string | undefined): number[];

export function createMock(options?: {
  delay?: number;
  bad?: boolean;
  /** 中文输入仍回中文词条，模拟没按提示词来的模型 */
  untranslated?: boolean;
  /** 截屏翻译的回复里故意漏掉这些编号（重发也漏） */
  drop?: number[];
  /** 截屏翻译的每个回复（补问也是）里最后一项都少一个 `}`、换行接 `]`（译文完整；真实模型偶尔这样写，这里是最坏情形） */
  unclosedLast?: boolean;
  log?: string | null;
}): MockServer;
