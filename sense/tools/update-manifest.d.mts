/** 给 TypeScript 看的类型声明：updateManifest.test.ts 要 import 这个脚本。 */
export const REPO: string;
export const PLATFORMS: string[];
export const ASSET_LABEL: Record<string, string>;
/** 某个平台、某个版本的安装包文件名，如 `SENSE_0.1.1_aarch64.app.tar.gz` */
export function assetName(version: string, platform: string): string;
export function releaseUrl(version: string, fileName: string): string;
/** 签名（.sig 的内容）里的可信注释字段（timestamp / file / version）；不验签，只读。格式不对返回 null */
export function signedFields(signature: string): Record<string, string> | null;
/** 签名绑定的版本号；没带 --app-version 或解不出来返回 null */
export function signedVersion(signature: string): string | null;

/** 用公钥（tauri.conf.json 里 plugins.updater.pubkey）给安装包验签；不抛 */
export function verifyMinisign(
  fileBytes: Uint8Array,
  signature: string,
  pubkey: string,
): { ok: true; fields: Record<string, string> } | { ok: false; reason: string };

export type ManifestAsset = { fileName: string; signature: string };
export type Manifest = {
  version: string;
  notes: string;
  pub_date: string;
  platforms: Record<string, { signature: string; url: string }>;
};

export function buildManifest(input: {
  version: string;
  notes?: string;
  pubDate?: Date;
  assets: Record<string, ManifestAsset>;
}): Manifest;

/** 返回问题列表，空数组 = 没问题 */
export function validateManifest(manifest: unknown, options?: { expectVersion?: string }): string[];

/** 给清单里的每个安装包用公钥验一次签，并核对验过的版本号、文件名；返回问题列表，空数组 = 全部通过 */
export function verifyAssets(
  manifest: unknown,
  pubkey: string,
  readAsset: (fileName: string) => Uint8Array,
): string[];

export function parseArgs(argv: string[]): {
  flags: Record<string, string | undefined>;
  assets: Record<string, string>;
  positional: string[];
};
