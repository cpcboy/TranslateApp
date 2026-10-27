/** 义项卡上的标签写法。纯数据与纯函数，便于单测：名单里新增一个值却忘了配标签，测试会红。 */

/** 名单外的词性不在这里：posLabel 会把模型给的原文直接显示出来，不替它改成别的 */
export const POS_LABEL: Record<string, string> = {
  n: "n.",
  v: "v.",
  adj: "adj.",
  adv: "adv.",
  prep: "prep.",
  conj: "conj.",
  pron: "pron.",
  num: "num.",
  phr: "phr.",
  interj: "interj.",
  aux: "aux.",
  det: "det.",
  art: "art.",
  part: "part.",
  abbr: "abbr.",
  other: "其他",
};

/** 只认自己的键：模型给 constructor / __proto__ 时不能读到原型上的东西 */
const own = (map: Record<string, string>, key: string): string | undefined =>
  Object.prototype.hasOwnProperty.call(map, key) ? map[key] : undefined;

export const posLabel = (pos: string): string => own(POS_LABEL, pos) ?? pos;

export const REGISTER_LABEL: Record<string, string> = {
  neutral: "",
  formal: "书面",
  informal: "口语",
  colloquial: "口语",
  technical: "技术",
  legal: "法律",
  slang: "俚语",
  vulgar: "粗俗",
  idiom: "习语",
  literary: "文学",
};

/** 名单外的语域（archaic 之类）原样显示 */
export const registerLabel = (register: string): string => own(REGISTER_LABEL, register) ?? register;

/** 学习者最容易踩雷的语域：不能淹没在灰色小字里 */
export const WARN_REGISTER: ReadonlySet<string> = new Set(["vulgar"]);

/** 例句下面的小标签：场景和领域常常同名（「法律 · 法律」），只在不同时才补领域 */
export function sceneLine(scene: string, domain: string | undefined): string {
  return domain && domain !== scene ? `${scene} · ${domain}` : scene;
}
