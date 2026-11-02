import { describe, expect, it } from "vitest";
import {
  assetName,
  buildManifest,
  parseArgs,
  PLATFORMS,
  releaseUrl,
  signedFields,
  signedVersion,
  validateManifest,
  verifyAssets,
  verifyMinisign,
  type Manifest,
} from "../../tools/update-manifest.mjs";
import tauriConf from "../../src-tauri/tauri.conf.json";

/**
 * 下面这些是 `tauri signer sign` 用**一次性测试密钥**真实签出来的（不是发布密钥，私钥没有留下）：
 * 每个向量带着被签的那几个字节（所以能真的验签）和文件名（可信注释里的 file:）。
 * 用真签名而不是自己拼的字符串，才能证明这里认得的格式、验得过的签名，就是 Tauri 实际写出来、应用实际会验的。
 */
const PUBKEY =
  "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDEyMEY3MTJEQTczQjQxMzkKUldRNVFUdW5MWEVQRWdmUjJlejFXUzM2MFgzYUEvN0p6TVpGa013Qm5OeFBTcUVJL0hkYzVyK3kK";
/** 另一把一次性测试密钥的公钥：用来证明别的密钥签的东西验不过 */
const OTHER_PUBKEY =
  "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IERGNEVCNUNFRTY2QkIzMUYKUldRZnMydm16clZPM3dtRmJTaXRoMG9FV2tPcTgxcWpKaGNnb0xVeUw5YXZzQ0E0ZjNBOHkveXIK";

type Vector = { name: string; text: string; signature: string };
/** 0.1.1，Apple 芯片，签的时候带了 --app-version 0.1.1 */
const AARCH64: Vector = {
  name: "SENSE_0.1.1_aarch64.app.tar.gz",
  text: "SENSE test package 0.1.1 aarch64\n",
  signature:
    "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRNVFUdW5MWEVQRWk2VFdyL2pvTFZIeTVoYVhHUzVvYk1SY0hEWWhhWFgydVdIcjIxMkwzOE93bit4WUMzcGt0STR6RWtod3k2UUhjb25sUlpNMEV2eTBxVDYxeUtUV2dBPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzNTI1Mjg4CWZpbGU6U0VOU0VfMC4xLjFfYWFyY2g2NC5hcHAudGFyLmd6CXZlcnNpb246MC4xLjEKdk0wd093Q3RXSU1YTjFHbnJaUVBERVQ0eVlRK2htUHJVQ29ibHFEdVBTSTVXMzFYM3E1UUNBdkZiQjZXQ1prZ3JydncvQmlSYndVbDJ1K2NqRjFEQnc9PQo=",
};
/** 0.1.1，Intel */
const X64: Vector = {
  name: "SENSE_0.1.1_x64.app.tar.gz",
  text: "SENSE test package 0.1.1 x64\n",
  signature:
    "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRNVFUdW5MWEVQRWhoRVVFUzhwUy9hUndnZ1M5OWRQWUJ3blY2SHQ5QmVBYitZUkxvQ1NickE1OFFuSXY1b29qZVJyVTBaVlpKN0hWSTJTU0JyL0VsbGRGVlhqTUF4aFEwPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzNTI1Mjg5CWZpbGU6U0VOU0VfMC4xLjFfeDY0LmFwcC50YXIuZ3oJdmVyc2lvbjowLjEuMQovT3kzcjc1V1pJRm5RUU9yWll5UlBhbnp6MG8vbXl5ejArTjB1Nzd6QzBYV3VZRkRWangwTjc0NFBIOTNEUm8wWXVnMGJwbGgrK2ZIUExScXBnZlpEUT09Cg==",
};
/** 0.1.2，Intel：用来造「签名绑的版本和清单不一致」 */
const X64_0_1_2: Vector = {
  name: "SENSE_0.1.2_x64.app.tar.gz",
  text: "SENSE test package 0.1.2 x64\n",
  signature:
    "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRNVFUdW5MWEVQRXZ4c1hROGxVd1pabVNBVVd6cXZxZC9aeFZWVFNSWkxGYnExeWczK2Z1WUFEKzNkZWEwZjFKZVBmTlNkREV5UjhwQzQ0Q1pzZHJhYnROTHJUMXMyQmdrPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzNTI1Mzk5CWZpbGU6U0VOU0VfMC4xLjJfeDY0LmFwcC50YXIuZ3oJdmVyc2lvbjowLjEuMgorN2dXZkk3K2o0N2RoYjU0TklGWHBNbm42ZDdiR21lbUhiOVhDZVJrTnRFaDBXZm1zMUd2SC9YQUNJalZTNWNqYXRGODFUUThrbUZvdVY4WElzK0JBUT09Cg==",
};
/** 签的时候没带 --app-version：可信注释里没有 version: */
const NO_VERSION: Vector = {
  name: "SENSE_0.1.1_nover.app.tar.gz",
  text: "SENSE test package 0.1.1 x64 without app version\n",
  signature:
    "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRNVFUdW5MWEVQRXVVMHVJNFdUc2F4QUpqZTlMRGRCZEt2akxSeW1sN2Y3U1pTOTBCRS9GcHNad25VMDVTRmRqNVg0bm5zWDRJRXhhYmIxdW5wdVJJczlzRklmMGRzSGdzPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzNTI1Mjg5CWZpbGU6U0VOU0VfMC4xLjFfbm92ZXIuYXBwLnRhci5negpCZEF2N3pxcFludUdrVzMxemFZaVpXTFVDWk1TT3c0d1ZsZTlJSnkwR0NMQUJjOWNwZmIvT1hla29JSi9iWk8rVHZ3SjQrZ0RjSm1NenI0MW05MWVDUT09Cg==",
};

const bytes = (vector: Vector) => new TextEncoder().encode(vector.text);

/** 清单里平台 → 它那个安装包（向量）。正常情况下，每个包的文件名就是 assetName */
const goodAssets = () => ({
  "darwin-aarch64": { fileName: AARCH64.name, signature: AARCH64.signature },
  "darwin-x86_64": { fileName: X64.name, signature: X64.signature },
});

const good = (): Manifest =>
  buildManifest({
    version: "0.1.1",
    notes: "修了几个 bug",
    pubDate: new Date("2026-11-01T04:08:08.123Z"),
    assets: goodAssets(),
  });

/** 把 .sig（base64 包着的文本）解开，改一改再包回去：造「被动过手脚的签名」 */
const tamperSignature = (signature: string, edit: (text: string) => string) => btoa(edit(atob(signature)));

/** 改外层文本里第 index 行的文字本身（那一行是一段 base64 文本）：造「内层行带空格」之类 */
const tamperLine = (outer: string, index: number, edit: (line: string) => string) =>
  tamperSignature(outer, (text) => {
    const lines = text.split("\n");
    lines[index] = edit(lines[index]);
    return lines.join("\n");
  });

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/** 同样的字节、非规范的写法：把末尾补位前那个字符的补位比特置 1（Buffer / atob 不在乎，Rust 的 base64 严格拒绝） */
const nonCanonical = (text: string) => {
  const pad = text.indexOf("=");
  const at = pad === -1 ? text.length - 1 : pad - 1;
  const bumped = B64[B64.indexOf(text[at]) | 1];
  return text.slice(0, at) + bumped + text.slice(at + 1);
};

/** 改外层文本里第 index 行（本身又是一段 base64）解开后的字节：造「长度不对的签名 / 公钥」 */
const tamperInner = (outer: string, index: number, edit: (binary: string) => string) =>
  tamperSignature(outer, (text) => {
    const lines = text.split("\n");
    lines[index] = btoa(edit(atob(lines[index])));
    return lines.join("\n");
  });

describe("签名里的字段", () => {
  it("认得出 Tauri 真签名的可信注释里的 file / version / timestamp", () => {
    expect(signedFields(AARCH64.signature)).toMatchObject({ file: AARCH64.name, version: "0.1.1" });
    expect(signedFields(AARCH64.signature)?.timestamp).toMatch(/^\d{10}$/);
    expect(signedVersion(X64.signature)).toBe("0.1.1");
    expect(signedVersion(X64_0_1_2.signature)).toBe("0.1.2");
  });

  it("和应用一样严格：清单里的 signature 前后多一个空白、内层行带空格、base64 补位比特不为零（非规范写法），都不认", () => {
    // 应用的 base64 解码（Rust base64 crate）对这些都是报错：Invalid symbol / Invalid encoding
    expect(signedVersion(`${X64.signature}\n`)).toBeNull();
    expect(signedVersion(` ${X64.signature}`)).toBeNull();
    expect(signedVersion(tamperLine(X64.signature, 1, (line) => `${line} `))).toBeNull();
    expect(signedVersion(nonCanonical(X64.signature))).toBeNull();
    expect(atob(nonCanonical(X64.signature))).toBe(atob(X64.signature)); // 确实是同样的字节，只是写法不规范
    const inner = tamperLine(X64.signature, 1, nonCanonical);
    expect(signedVersion(inner)).toBeNull();
    // 对照：规范的照常认
    expect(signedVersion(X64.signature)).toBe("0.1.1");
  });

  it("可信注释里同名字段取第一个，和应用一样（后一个覆盖前一个的话，这里读到的和应用读到的不是同一个）", () => {
    const doubled = tamperSignature(AARCH64.signature, (text) => text.replace("version:0.1.1", "version:0.1.1\tversion:0.9.9"));
    expect(signedVersion(doubled)).toBe("0.1.1");
    const reversed = tamperSignature(AARCH64.signature, (text) => text.replace("version:0.1.1", "version:0.9.9\tversion:0.1.1"));
    expect(signedVersion(reversed)).toBe("0.9.9");
    // 字段名前面带空格不算同名
    const spaced = tamperSignature(AARCH64.signature, (text) => text.replace("\tversion:0.1.1", "\t version:0.1.1"));
    expect(signedVersion(spaced)).toBeNull();
  });

  it("签的时候没带 --app-version → 有字段但没有版本", () => {
    expect(signedFields(NO_VERSION.signature)).toMatchObject({ file: NO_VERSION.name });
    expect(signedVersion(NO_VERSION.signature)).toBeNull();
  });

  it("乱码、格式不对的一律返回 null，不抛：空、不是 base64、行数不对、签名长度不对、缺可信注释", () => {
    expect(signedFields("")).toBeNull();
    expect(signedFields("%%% 不是 base64 %%%")).toBeNull();
    expect(signedVersion(btoa("untrusted comment: x\nRWQ=\n"))).toBeNull();
    // 夹了空白的 base64：Buffer.from 会悄悄丢掉脏字符，这里必须拒绝
    expect(signedFields(`${X64.signature.slice(0, 40)} ${X64.signature.slice(40)}`)).toBeNull();
    // 少一行 / 多一行
    expect(signedFields(tamperSignature(X64.signature, (text) => text.split("\n").slice(0, 3).join("\n")))).toBeNull();
    expect(signedFields(tamperSignature(X64.signature, (text) => `${text}extra line\n`))).toBeNull();
    // 可信注释那行的前缀不对
    expect(signedFields(tamperSignature(X64.signature, (text) => text.replace(/\ntrusted comment: /, "\ntrusted comment:")))).toBeNull();
    for (const junk of [null, undefined, 42, {}, []]) {
      expect(signedFields(junk as unknown as string)).toBeNull();
    }
  });

  it("签名本体的长度不对一律不认（minisign 的签名固定 74 字节；抄漏一个字符的签名就是这样）", () => {
    expect(signedFields(tamperInner(X64.signature, 1, (binary) => binary.slice(0, -1)))).toBeNull();
    expect(signedFields(tamperInner(X64.signature, 1, (binary) => `${binary}x`))).toBeNull();
    // 算法标记既不是 Ed 也不是 ED
    expect(signedFields(tamperInner(X64.signature, 1, (binary) => `XX${binary.slice(2)}`))).toBeNull();
    // 全局签名的长度不对
    expect(signedFields(tamperInner(X64.signature, 3, (binary) => binary.slice(1)))).toBeNull();
  });
});

describe("验签（用应用里写死的公钥）", () => {
  it("真签名用对的公钥验得过，并带回被签名确认过的 file / version", () => {
    for (const vector of [AARCH64, X64, X64_0_1_2]) {
      const result = verifyMinisign(bytes(vector), vector.signature, PUBKEY);
      expect(result.ok, vector.name).toBe(true);
      if (result.ok) expect(result.fields.file).toBe(vector.name);
    }
    const result = verifyMinisign(bytes(AARCH64), AARCH64.signature, PUBKEY);
    expect(result.ok && result.fields.version).toBe("0.1.1");
  });

  it("另一把密钥签的东西验不过（密钥 ID 对不上）", () => {
    const result = verifyMinisign(bytes(AARCH64), AARCH64.signature, OTHER_PUBKEY);
    expect(result).toEqual({ ok: false, reason: expect.stringContaining("密钥 ID") });
  });

  it("文件被改过一个字节、被追加、被截断、整个换成另一个包，都验不过", () => {
    const original = bytes(AARCH64);
    const flipped = Uint8Array.from(original);
    flipped[0] ^= 1;
    for (const tampered of [flipped, new Uint8Array([...original, 0]), original.slice(0, -1), new Uint8Array(), bytes(X64)]) {
      const result = verifyMinisign(tampered, AARCH64.signature, PUBKEY);
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.reason).toContain("文件内容");
    }
  });

  it("签名里的版本号 / 文件名被改过（想把旧版本的包冒充成新版本）：全局签名挡住", () => {
    for (const edit of [
      (text: string) => text.replace("version:0.1.1", "version:0.9.9"),
      (text: string) => text.replace(AARCH64.name, "SENSE_9.9.9_aarch64.app.tar.gz"),
      (text: string) => text.replace(/version:0\.1\.1/, "version:0.1.1\tversion:9.9.9"),
    ]) {
      const forged = tamperSignature(AARCH64.signature, edit);
      expect(forged).not.toBe(AARCH64.signature);
      const result = verifyMinisign(bytes(AARCH64), forged, PUBKEY);
      expect(result.ok).toBe(false);
    }
    // 改版本号这一种，具体说的是「注释被改过」
    const forged = tamperSignature(AARCH64.signature, (text) => text.replace("version:0.1.1", "version:0.9.9"));
    const result = verifyMinisign(bytes(AARCH64), forged, PUBKEY);
    expect(result).toEqual({ ok: false, reason: expect.stringContaining("注释") });
  });

  it("把另一个包的签名贴过来（文件内容和签名对不上）验不过", () => {
    expect(verifyMinisign(bytes(AARCH64), X64.signature, PUBKEY).ok).toBe(false);
  });

  it("应用会拒的写法这里也拒（不能比应用宽松，否则「验过了」就不等于「装机的电脑会放行」）：signature 末尾带换行、内层行带空格、非规范 base64；公钥同理", () => {
    const refused = (signature: string, pubkey = PUBKEY) => verifyMinisign(bytes(X64), signature, pubkey);
    expect(refused(`${X64.signature}\n`)).toEqual({ ok: false, reason: expect.stringContaining("签名格式") });
    expect(refused(tamperLine(X64.signature, 1, (line) => `${line} `))).toEqual({ ok: false, reason: expect.stringContaining("签名格式") });
    expect(refused(nonCanonical(X64.signature))).toEqual({ ok: false, reason: expect.stringContaining("签名格式") });
    expect(refused(X64.signature, `${PUBKEY}\n`)).toEqual({ ok: false, reason: expect.stringContaining("公钥格式") });
    expect(refused(X64.signature, nonCanonical(PUBKEY))).toEqual({ ok: false, reason: expect.stringContaining("公钥格式") });
    expect(refused(X64.signature).ok).toBe(true);
  });

  it("公钥长度不对（抄漏了字符）：说公钥格式不对，而不是验签时抛出一个不知所云的错", () => {
    expect(verifyMinisign(bytes(X64), X64.signature, tamperInner(PUBKEY, 1, (binary) => binary.slice(0, -1)))).toEqual({
      ok: false,
      reason: expect.stringContaining("公钥格式"),
    });
    expect(verifyMinisign(bytes(X64), X64.signature, tamperInner(PUBKEY, 1, (binary) => `${binary}x`))).toEqual({
      ok: false,
      reason: expect.stringContaining("公钥格式"),
    });
  });

  it("公钥 / 签名格式不对：说清楚是哪个不对，不抛", () => {
    expect(verifyMinisign(bytes(X64), X64.signature, "REPLACE_WITH_CONTENTS_OF_sense-updater.key.pub")).toEqual({
      ok: false,
      reason: expect.stringContaining("公钥格式"),
    });
    expect(verifyMinisign(bytes(X64), X64.signature, "")).toEqual({ ok: false, reason: expect.stringContaining("公钥格式") });
    expect(verifyMinisign(bytes(X64), "垃圾", PUBKEY)).toEqual({ ok: false, reason: expect.stringContaining("签名格式") });
    expect(verifyMinisign(bytes(X64), "", PUBKEY)).toEqual({ ok: false, reason: expect.stringContaining("签名格式") });
    for (const junk of [null, undefined, 42]) {
      expect(verifyMinisign(bytes(X64), junk as unknown as string, junk as unknown as string).ok).toBe(false);
    }
  });
});

describe("生成清单", () => {
  it("格式就是 Tauri updater 要的：version / notes / pub_date / platforms，地址指向这个版本的 Release", () => {
    const manifest = good();
    expect(manifest).toEqual({
      version: "0.1.1",
      notes: "修了几个 bug",
      pub_date: "2026-11-01T04:08:08Z",
      platforms: {
        "darwin-aarch64": {
          signature: AARCH64.signature,
          url: "https://github.com/cpcboy/TranslateApp/releases/download/v0.1.1/SENSE_0.1.1_aarch64.app.tar.gz",
        },
        "darwin-x86_64": {
          signature: X64.signature,
          url: "https://github.com/cpcboy/TranslateApp/releases/download/v0.1.1/SENSE_0.1.1_x64.app.tar.gz",
        },
      },
    });
    expect(releaseUrl("1.2.3", "a b.tar.gz")).toBe("https://github.com/cpcboy/TranslateApp/releases/download/v1.2.3/a%20b.tar.gz");
  });

  it("安装包的文件名约定：SENSE_<版本>_<aarch64|x64>.app.tar.gz（release.sh 打包用的就是这个）", () => {
    expect(assetName("0.1.1", "darwin-aarch64")).toBe("SENSE_0.1.1_aarch64.app.tar.gz");
    expect(assetName("0.1.1", "darwin-x86_64")).toBe("SENSE_0.1.1_x64.app.tar.gz");
  });

  it("两种芯片的包都要有：少一种就拒绝生成（那种芯片的电脑会永远收不到更新）", () => {
    for (const missing of PLATFORMS) {
      const assets = goodAssets() as Record<string, { fileName: string; signature: string }>;
      delete assets[missing];
      expect(() => buildManifest({ version: "0.1.1", assets }), missing).toThrow(missing);
    }
  });

  it("拒绝：版本号不是 x.y.z、签名为空或带空白、文件名带路径、不认识的平台", () => {
    const ok = goodAssets();
    expect(() => buildManifest({ version: "v0.1.1", assets: ok })).toThrow("x.y.z");
    expect(() => buildManifest({ version: "0.1", assets: ok })).toThrow("x.y.z");
    expect(() => buildManifest({ version: "0.1.1", assets: { ...ok, "darwin-x86_64": { fileName: "b", signature: "" } } })).toThrow("签名");
    expect(() => buildManifest({ version: "0.1.1", assets: { ...ok, "darwin-x86_64": { fileName: "b", signature: "ab cd" } } })).toThrow("签名");
    expect(() => buildManifest({ version: "0.1.1", assets: { ...ok, "darwin-x86_64": { fileName: "dir/b", signature: "x" } } })).toThrow("文件名");
    expect(() => buildManifest({ version: "0.1.1", assets: { ...ok, "windows-x86_64": { fileName: "w", signature: "x" } } })).toThrow("windows-x86_64");
  });

  it("没写更新说明（或只有空白）就用「SENSE v版本号」，不留空：应用要把它显示出来", () => {
    expect(buildManifest({ version: "0.1.1", notes: "  \n", assets: goodAssets() }).notes).toBe("SENSE v0.1.1");
    expect(buildManifest({ version: "0.1.1", assets: goodAssets() }).notes).toBe("SENSE v0.1.1");
  });
});

describe("核对清单", () => {
  it("正常的清单没有问题", () => {
    expect(validateManifest(good(), { expectVersion: "0.1.1" })).toEqual([]);
  });

  it("版本号和预期不符", () => {
    expect(validateManifest(good(), { expectVersion: "0.1.2" })).toEqual(["version 是 0.1.1，应当是 0.1.2"]);
  });

  it("签名绑的版本和清单写的不一致（应用会拒收，发布前必须发现）", () => {
    const manifest = good();
    manifest.platforms["darwin-x86_64"].signature = X64_0_1_2.signature;
    const problems = validateManifest(manifest);
    expect(problems.some((item) => item.includes("darwin-x86_64") && item.includes("绑的是 0.1.2"))).toBe(true);
  });

  it("签名里根本没有版本号（签名时漏了 --app-version）", () => {
    const manifest = good();
    manifest.platforms["darwin-aarch64"].signature = NO_VERSION.signature;
    const problems = validateManifest(manifest);
    expect(problems.some((item) => item.includes("darwin-aarch64") && item.includes("没有版本号"))).toBe(true);
  });

  it("签名是另一个包的（Apple 芯片的签名贴到了 Intel 那条上）：版本对得上也要拦下", () => {
    const manifest = good();
    manifest.platforms["darwin-x86_64"].signature = AARCH64.signature;
    const problems = validateManifest(manifest);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("darwin-x86_64");
    expect(problems[0]).toContain(AARCH64.name);
  });

  it("签名乱码：说签名格式不对", () => {
    const manifest = good();
    manifest.platforms["darwin-aarch64"].signature = "乱码";
    expect(validateManifest(manifest).some((item) => item.includes("darwin-aarch64") && item.includes("签名格式"))).toBe(true);
  });

  const withUrl = (platform: "darwin-aarch64" | "darwin-x86_64", url: unknown) => {
    const manifest = good();
    (manifest.platforms[platform] as { url: unknown }).url = url;
    return validateManifest(manifest);
  };
  const base = "https://github.com/cpcboy/TranslateApp/releases/download/v0.1.1/";

  it("地址必须恰好是这个版本 Release 里这一种芯片的那个文件：别的主机 / 协议 / 仓库 / 标签 / 目录穿越 都拦下", () => {
    for (const url of [
      "https://example.com/x.app.tar.gz",
      `http://github.com/cpcboy/TranslateApp/releases/download/v0.1.1/${AARCH64.name}`,
      `https://github.com.evil.com/cpcboy/TranslateApp/releases/download/v0.1.1/${AARCH64.name}`,
      `https://github.com@evil.com/cpcboy/TranslateApp/releases/download/v0.1.1/${AARCH64.name}`,
      `https://user:pw@github.com/cpcboy/TranslateApp/releases/download/v0.1.1/${AARCH64.name}`,
      `https://github.com:444/cpcboy/TranslateApp/releases/download/v0.1.1/${AARCH64.name}`,
      `https://github.com/someone-else/TranslateApp/releases/download/v0.1.1/${AARCH64.name}`,
      `https://github.com/cpcboy/TranslateApp/releases/download/v0.1.0/${AARCH64.name}`,
      `${base}../v0.1.0/${AARCH64.name}`,
      `${base}%2e%2e/v0.1.0/${AARCH64.name}`,
      `${base}..%2f..%2fv0.1.0%2f${AARCH64.name}`,
    ]) {
      const problems = withUrl("darwin-aarch64", url);
      expect(problems.length, url).toBeGreaterThan(0);
      expect(problems.every((item) => item.includes("darwin-aarch64")), url).toBe(true);
    }
  });

  it("地址在对的目录下，但文件不对：不是 .app.tar.gz、别的芯片的包、带 ? 或 #、不是网址、没有 url", () => {
    expect(withUrl("darwin-x86_64", `${base}x.zip`).some((item) => item.includes(".app.tar.gz"))).toBe(true);
    // Apple 芯片那条指到了 Intel 的包：Apple 芯片的电脑会下到装不起来的包
    expect(withUrl("darwin-aarch64", `${base}${X64.name}`).some((item) => item.includes(`应当是 ${AARCH64.name}`))).toBe(true);
    expect(withUrl("darwin-aarch64", `${base}${AARCH64.name}?x=1`).some((item) => item.includes("? 或 #"))).toBe(true);
    expect(withUrl("darwin-aarch64", `${base}${AARCH64.name}#x`).some((item) => item.includes("? 或 #"))).toBe(true);
    expect(withUrl("darwin-aarch64", "不是网址").some((item) => item.includes("不是合法的网址"))).toBe(true);
    expect(withUrl("darwin-aarch64", undefined).some((item) => item.includes("没有 url"))).toBe(true);
    expect(withUrl("darwin-aarch64", `${base}${AARCH64.name}`)).toEqual([]);
  });

  it("pub_date 必须是 RFC 3339（Tauri 用 Rust 的解析器读它，只有日期、缺时区的写法它不认）", () => {
    for (const bad of ["昨天", "2026-11-01", "2026-11-01 04:08:08", "2026-11-01T04:08:08", "2026-13-45T25:61:61Z", "", 20261101]) {
      expect(validateManifest({ ...good(), pub_date: bad }).some((item) => item.includes("pub_date")), String(bad)).toBe(true);
    }
    for (const fine of ["2026-11-01T04:08:08Z", "2026-11-01T12:08:08+08:00", "2026-11-01T04:08:08.123Z"]) {
      expect(validateManifest({ ...good(), pub_date: fine }), fine).toEqual([]);
    }
  });

  it("缺平台 / 多平台 / 说明是空的 / 根本不是对象", () => {
    const missing = good();
    delete (missing.platforms as Record<string, unknown>)["darwin-x86_64"];
    expect(validateManifest(missing)).toContain("缺 darwin-x86_64");

    const notObject = good();
    (notObject.platforms as Record<string, unknown>)["darwin-x86_64"] = "https://x";
    expect(validateManifest(notObject)).toContain("缺 darwin-x86_64");

    const extra = good();
    (extra.platforms as Record<string, unknown>)["linux-x86_64"] = { url: "x", signature: "y" };
    expect(validateManifest(extra).some((item) => item.includes("linux-x86_64"))).toBe(true);

    expect(validateManifest({ ...good(), notes: " " }).some((item) => item.includes("notes"))).toBe(true);
    expect(validateManifest(null)).toEqual(["清单不是 JSON 对象"]);
    expect(validateManifest("x")).toEqual(["清单不是 JSON 对象"]);
    expect(validateManifest({ version: "x" }).length).toBeGreaterThan(1);
    expect(validateManifest({ ...good(), platforms: null }).length).toBeGreaterThan(0);
  });

  it("应用里写死的端点（tauri.conf.json）指向的仓库，就是清单里地址用的仓库", () => {
    // 两边各写了一遍仓库名；改了一边忘了另一边，应用会去一个永远没有清单的地方找更新
    const [endpoint] = (tauriConf.plugins.updater.endpoints ?? []) as string[];
    expect(endpoint).toMatch(/^https:\/\/github\.com\/cpcboy\/TranslateApp\/releases\/latest\/download\/latest\.json$/);
    expect(releaseUrl("0.1.1", "x")).toContain("https://github.com/cpcboy/TranslateApp/releases/download/");
  });
});

describe("给清单里的安装包逐个验签", () => {
  const files: Record<string, Uint8Array> = {
    [AARCH64.name]: bytes(AARCH64),
    [X64.name]: bytes(X64),
  };
  const read = (name: string) => {
    const content = files[name];
    if (!content) throw new Error(`没有 ${name}`);
    return content;
  };

  it("两个包都是对的密钥签的、内容没动、版本和文件名都对得上：没有问题", () => {
    expect(verifyAssets(good(), PUBKEY, read)).toEqual([]);
  });

  it("换了一把公钥（应用里写死的和签名用的不是一把）：两个包都报", () => {
    const problems = verifyAssets(good(), OTHER_PUBKEY, read);
    expect(problems).toHaveLength(2);
    expect(problems.every((item) => item.includes("密钥 ID"))).toBe(true);
  });

  it("下载下来的包被换了（内容变了）：报出是哪个平台、哪个文件", () => {
    const problems = verifyAssets(good(), PUBKEY, (name) => (name === X64.name ? new TextEncoder().encode("被换掉了") : read(name)));
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("darwin-x86_64");
    expect(problems[0]).toContain(X64.name);
    expect(problems[0]).toContain("文件内容");
  });

  it("读不到安装包（没上传上去 / 下载失败）", () => {
    const problems = verifyAssets(good(), PUBKEY, (name) => {
      if (name === AARCH64.name) throw new Error("ENOENT");
      return read(name);
    });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("读不到");
  });

  it("验得过但签名绑的版本 / 文件名和清单不符：照样报（验过的才算数，不是清单自己说了算）", () => {
    // 清单（0.1.1）里 Intel 那条挂着 0.1.2 的包和签名：签名本身是真的，但绑的是 0.1.2
    const manifest = good();
    manifest.platforms["darwin-x86_64"].signature = X64_0_1_2.signature;
    const problems = verifyAssets(manifest, PUBKEY, (name) => (name === X64.name ? bytes(X64_0_1_2) : read(name)));
    expect(problems.some((item) => item.includes("绑的是 0.1.2"))).toBe(true);
    expect(problems.some((item) => item.includes(`签的是 ${X64_0_1_2.name}`))).toBe(true);
  });

  it("清单本身残缺：缺平台、地址不是网址、不是对象", () => {
    const missing = good();
    delete (missing.platforms as Record<string, unknown>)["darwin-aarch64"];
    expect(verifyAssets(missing, PUBKEY, read)).toContain("缺 darwin-aarch64");
    const badUrl = good();
    badUrl.platforms["darwin-aarch64"].url = "不是网址";
    expect(verifyAssets(badUrl, PUBKEY, read).some((item) => item.includes("不是合法的网址"))).toBe(true);
    expect(verifyAssets(null, PUBKEY, read)).toEqual(["清单不是 JSON 对象"]);
  });
});

describe("命令行参数", () => {
  // 读写文件的那一层（build 读 .sig、写 latest.json；check / verify 读文件或标准输入）由真跑 release.sh 验证：
  // 项目不引入 Node 类型依赖，单测里碰不了文件系统
  it("--asset 可以重复，平台与路径用第一个 = 分开（路径里带 = 也不怕）；其余 --名字 值 进 flags；剩下的是位置参数", () => {
    const parsed = parseArgs([
      "build",
      "--version", "0.1.1",
      "--asset", "darwin-aarch64=dist/a b.app.tar.gz",
      "--asset", "darwin-x86_64=dist/x=1.app.tar.gz",
      "--out", "latest.json",
      "-",
    ]);
    expect(parsed.flags).toEqual({ version: "0.1.1", out: "latest.json" });
    expect(parsed.assets).toEqual({
      "darwin-aarch64": "dist/a b.app.tar.gz",
      "darwin-x86_64": "dist/x=1.app.tar.gz",
    });
    expect(parsed.positional).toEqual(["build", "-"]);
  });

  it("verify 的参数：位置参数是清单，--dir / --pubkey / --version 进 flags", () => {
    const parsed = parseArgs(["latest.json", "--dir", "/tmp/x", "--pubkey", "@k.pub", "--version", "0.1.1"]);
    expect(parsed.positional).toEqual(["latest.json"]);
    expect(parsed.flags).toEqual({ dir: "/tmp/x", pubkey: "@k.pub", version: "0.1.1" });
  });

  it("没有参数也不抛", () => {
    expect(parseArgs([])).toEqual({ flags: {}, assets: {}, positional: [] });
  });
});
