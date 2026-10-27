/** 给 TypeScript 看的类型声明：mockProvider.test.ts 要 import 这个 mock。 */
export const MARKERS: {
  capture: string;
  longText: string;
  grammar: string;
  explain: string;
};

export type MockServer = {
  listen(port: number, host: string, callback?: () => void): unknown;
  close(callback?: () => void): unknown;
  address(): { port: number } | string | null;
};

export function createMock(options?: { delay?: number; bad?: boolean; log?: string | null }): MockServer;
