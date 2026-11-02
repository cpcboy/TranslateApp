#!/usr/bin/env node
/**
 * 生成 / 核对 / 验签自动更新用的清单 latest.json（Tauri updater 的静态清单格式）。
 *
 * 用法：
 *   node tools/update-manifest.mjs build --version 0.1.1 [--notes-file notes.md] --out latest.json \
 *        --asset darwin-aarch64=dist-release/SENSE_0.1.1_aarch64.app.tar.gz \
 *        --asset darwin-x86_64=dist-release/SENSE_0.1.1_x64.app.tar.gz
 *   node tools/update-manifest.mjs check <latest.json | -> [--version 0.1.1]     （- = 从标准输入读）
 *   node tools/update-manifest.mjs verify <latest.json | -> --dir <放着安装包的目录> --pubkey <公钥 | @公钥文件> [--version 0.1.1]
 *
 * `--asset` 的文件旁边必须有同名的 `.sig`（`tauri signer sign` 的产物）：清单里的 signature 就是它的内容。
 * 下载地址不用手写：固定是 GitHub Release 的 `releases/download/v<版本>/<文件名>`，与 release.sh 上传的位置一致。
 *
 * `check`（不联网、不读安装包）逐条核对清单本身：版本号、两种芯片都在、地址**恰好**是这个版本 Release 里
 * 这一种芯片的那个文件（不是别的路径 / 别的文件名）、签名里绑定的版本号和文件名都对得上
 * （应用开着 `requireSignedVersion`，签名里没有版本号的安装包会被应用拒收）。
 *
 * `verify` 在 `check` 之外，用**应用里写死的那把公钥**对每个安装包真的验一次签：
 * 文件内容、版本号、文件名都是被签名覆盖的，验过了才说明装机的电脑也会放行。
 * 有问题就列出来并以非 0 退出。
 */
import { readFileSync, writeFileSync, realpathSync } from "node:fs";
import { createHash, createPublicKey, verify } from "node:crypto";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";

export const REPO = "cpcboy/TranslateApp";

/** 必须出现在清单里的平台：Apple 芯片与 Intel。键名是 Tauri updater 规定的 `<os>-<arch>`。 */
export const PLATFORMS = ["darwin-aarch64", "darwin-x86_64"];

/** 平台 → 安装包文件名里的芯片标签（release.sh 打包时用的就是这个）。 */
export const ASSET_LABEL = { "darwin-aarch64": "aarch64", "darwin-x86_64": "x64" };

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

/** RFC 3339 的日期时间（Tauri 用 Rust 的 RFC 3339 解析器读它：只有日期、缺时区的写法它不认，JS 的 Date.parse 却认）。 */
const RFC3339 = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/;

export function assetName(version, platform) {
  return `SENSE_${version}_${ASSET_LABEL[platform]}.app.tar.gz`;
}

export function releaseUrl(version, fileName) {
  return `https://github.com/${REPO}/releases/download/v${version}/${encodeURIComponent(fileName)}`;
}

// ---------- minisign 的签名 / 公钥 ----------

const BASE64 = /^[A-Za-z0-9+/]+={0,2}$/;

/**
 * 和应用里的 base64 解码（Rust 的 base64 crate，STANDARD 引擎）一样严格：
 * 不去首尾空白（清单里 signature 末尾多一个换行，应用会报 Invalid symbol）、不接受别的字符、
 * 补位不对、末尾补位比特不为零的「非规范」写法也不接受（Buffer.from 对这些都是静默放过的）。
 * 判断办法：能解码、而且重新编码回去必须逐字等于原串。
 */
function decodeBase64(text) {
  if (typeof text !== "string" || text.length === 0 || text.length % 4 !== 0 || !BASE64.test(text)) return null;
  const bytes = Buffer.from(text, "base64");
  return bytes.toString("base64") === text ? bytes : null;
}

/**
 * 可信注释是制表符分隔的 `key:value`，如 `timestamp:…\tfile:…\tversion:0.1.1`。
 * 同名字段取**第一个**——应用（minisign 的校验库 + 插件）就是这么读的：`split('\t').find_map(strip_prefix("version:"))`；
 * 后一个覆盖前一个的话，注释里塞两个 `version:` 时这里读到的和应用读到的就不是同一个。
 */
function parseFields(comment) {
  const fields = {};
  for (const item of comment.split("\t")) {
    const at = item.indexOf(":");
    if (at <= 0) continue;
    const key = item.slice(0, at);
    if (!(key in fields)) fields[key] = item.slice(at + 1);
  }
  return fields;
}

/**
 * 拆开 `.sig` 文件的内容（base64 包着的 minisign 签名，正好四行：
 * 不可信注释 / 签名 / 可信注释 / 对「签名 + 可信注释」的全局签名）。格式不对返回 null。
 * 验签（`verifyMinisign`）和读版本号（`signedFields`）用同一个解析，不会出现「读到的和验的不是同一行」。
 */
function parseSignature(signature) {
  const bytes = decodeBase64(signature);
  if (!bytes) return null;
  const lines = bytes.toString("utf8").replace(/\n$/, "").split("\n");
  if (lines.length !== 4) return null;
  if (!lines[0].startsWith("untrusted comment:") || !lines[2].startsWith("trusted comment: ")) return null;
  const body = decodeBase64(lines[1]);
  const global = decodeBase64(lines[3]);
  if (!body || body.length !== 74 || !global || global.length !== 64) return null;
  const algorithm = body.toString("latin1", 0, 2);
  if (algorithm !== "Ed" && algorithm !== "ED") return null;
  const trustedComment = lines[2].slice("trusted comment: ".length);
  return {
    // "ED" = 先对文件做 BLAKE2b-512 再签（minisign 的默认）；"Ed" = 直接签文件（老格式）
    algorithm,
    keyId: body.subarray(2, 10),
    signature: body.subarray(10),
    trustedComment,
    globalSignature: global,
    fields: parseFields(trustedComment),
  };
}

/**
 * 签名里被签名覆盖的可信注释字段（`timestamp` / `file` / `version`）。注意：这里**不验签**，只是读；
 * 要确认它是真的，用 `verifyMinisign`。格式不对返回 null。
 */
export function signedFields(signature) {
  return parseSignature(signature)?.fields ?? null;
}

/** 签名绑定的版本号；签的时候没带 --app-version、或解不出来返回 null。 */
export function signedVersion(signature) {
  return signedFields(signature)?.version ?? null;
}

const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

/** `tauri.conf.json` 里的 pubkey：base64 包着整个 minisign 公钥文件（第二行是 base64 的 `Ed` + 密钥 ID + 32 字节公钥）。 */
function parsePublicKey(pubkey) {
  const bytes = decodeBase64(pubkey);
  if (!bytes) return null;
  const lines = bytes.toString("utf8").replace(/\n$/, "").split("\n");
  if (lines.length !== 2 || !lines[0].startsWith("untrusted comment:")) return null;
  const raw = decodeBase64(lines[1]);
  if (!raw || raw.length !== 42 || raw.toString("latin1", 0, 2) !== "Ed") return null;
  return {
    keyId: raw.subarray(2, 10),
    key: createPublicKey({ key: Buffer.concat([ED25519_SPKI_PREFIX, raw.subarray(10)]), format: "der", type: "spki" }),
  };
}

/**
 * 用公钥给一个安装包验签，做的和应用里的 minisign-verify 一样的两步：
 * ① 签名覆盖文件内容；② 全局签名覆盖「签名 + 可信注释」，所以注释里的版本号、文件名也改不了。
 * 不抛：任何格式问题、验不过都是 `{ ok: false, reason }`。验过了返回被签名确认过的注释字段。
 *
 * @param {Uint8Array} fileBytes 安装包内容
 * @param {string} signature `.sig` 文件的内容
 * @param {string} pubkey tauri.conf.json 里 plugins.updater.pubkey 那一串
 * @returns {{ok: true, fields: Record<string, string>} | {ok: false, reason: string}}
 */
export function verifyMinisign(fileBytes, signature, pubkey) {
  try {
    const pub = parsePublicKey(pubkey);
    if (!pub) return { ok: false, reason: "公钥格式不对（应当是 tauri.conf.json 里 plugins.updater.pubkey 那一串）" };
    const sig = parseSignature(signature);
    if (!sig) return { ok: false, reason: "签名格式不对（应当是 tauri signer sign 生成的 .sig 的内容）" };
    if (!sig.keyId.equals(pub.keyId)) return { ok: false, reason: "签名是另一把密钥签的（密钥 ID 对不上）" };
    const message = sig.algorithm === "ED" ? createHash("blake2b512").update(fileBytes).digest() : Buffer.from(fileBytes);
    if (!verify(null, message, pub.key, sig.signature)) {
      return { ok: false, reason: "文件内容和签名对不上（文件被改过，或不是这把密钥签的）" };
    }
    const covered = Buffer.concat([sig.signature, Buffer.from(sig.trustedComment, "utf8")]);
    if (!verify(null, covered, pub.key, sig.globalSignature)) {
      return { ok: false, reason: "签名里的注释（版本号、文件名）被改过" };
    }
    return { ok: true, fields: sig.fields };
  } catch (err) {
    return { ok: false, reason: `验签时出错：${err instanceof Error ? err.message : String(err)}` };
  }
}

// ---------- 清单 ----------

/** 清单里的 `pub_date`：RFC 3339，去掉毫秒（Tauri 的解析器两种都认，去掉更干净）。 */
function rfc3339(date) {
  return date.toISOString().replace(/\.\d{3}Z$/, "Z");
}

/**
 * @param {{version: string, notes?: string, pubDate?: Date, assets: Record<string, {fileName: string, signature: string}>}} input
 */
export function buildManifest({ version, notes, pubDate = new Date(), assets }) {
  if (!SEMVER.test(version)) throw new Error(`版本号不是 x.y.z：${version}`);
  const platforms = {};
  for (const platform of PLATFORMS) {
    const asset = assets[platform];
    if (!asset) throw new Error(`缺 ${platform} 的安装包：两种芯片都要有，少一种那种芯片的电脑就收不到更新`);
    if (!asset.fileName || asset.fileName.includes("/")) throw new Error(`${platform} 的文件名不对：${asset.fileName}`);
    const signature = String(asset.signature ?? "").trim();
    if (!signature || /\s/.test(signature)) throw new Error(`${platform} 的签名为空或带空白，应当是 .sig 文件里那一行`);
    platforms[platform] = { signature, url: releaseUrl(version, asset.fileName) };
  }
  for (const platform of Object.keys(assets)) {
    if (!PLATFORMS.includes(platform)) throw new Error(`不认识的平台 ${platform}（只支持 ${PLATFORMS.join("、")}）`);
  }
  const trimmed = (notes ?? "").trim();
  return {
    version,
    notes: trimmed || `SENSE v${version}`,
    pub_date: rfc3339(pubDate),
    platforms,
  };
}

/** 清单里一个地址：必须**恰好**是这个版本的 Release 里这一种芯片的安装包。返回问题列表。 */
function checkUrl(platform, url, version) {
  const expected = assetName(version, platform);
  const prefix = `https://github.com/${REPO}/releases/download/v${version}/`;
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return [`${platform} 的地址不是合法的网址：${url}`];
  }
  const directory = `/${REPO}/releases/download/v${version}/`;
  // new URL 已经把 .. 和 %2e%2e 归一掉了，所以「路径以 directory 开头」就是真的在这个目录下
  if (parsed.protocol !== "https:" || parsed.host !== "github.com" || parsed.username || parsed.password || !parsed.pathname.startsWith(directory)) {
    return [`${platform} 的地址不在 ${prefix} 下：${url}`];
  }
  const problems = [];
  const name = decodeURIComponent(parsed.pathname.slice(directory.length));
  if (!name.endsWith(".app.tar.gz")) problems.push(`${platform} 的地址不是 .app.tar.gz：${url}`);
  else if (name !== expected) problems.push(`${platform} 的安装包文件名应当是 ${expected}，清单里是 ${name}`);
  if (parsed.search || parsed.hash) problems.push(`${platform} 的地址带了多余的 ? 或 #：${url}`);
  return problems;
}

/**
 * 核对一份清单，返回问题列表（空 = 没问题）。`expectVersion` 给了就还要核对版本号。
 * 不联网、不读安装包：只看清单自己和它引用的签名（签名是不是真的，用 `verifyAssets`）。
 */
export function validateManifest(manifest, { expectVersion } = {}) {
  const problems = [];
  if (!manifest || typeof manifest !== "object") return ["清单不是 JSON 对象"];
  const { version } = manifest;
  if (typeof version !== "string" || !SEMVER.test(version)) problems.push(`version 不是 x.y.z：${version}`);
  if (expectVersion && version !== expectVersion) problems.push(`version 是 ${version}，应当是 ${expectVersion}`);
  if (typeof manifest.pub_date !== "string" || !RFC3339.test(manifest.pub_date) || Number.isNaN(Date.parse(manifest.pub_date))) {
    problems.push(`pub_date 不是合法的 RFC 3339 时间（形如 2026-11-01T04:08:08Z）：${manifest.pub_date}`);
  }
  if (typeof manifest.notes !== "string" || !manifest.notes.trim()) problems.push("notes 是空的");

  const platforms = manifest.platforms && typeof manifest.platforms === "object" ? manifest.platforms : {};
  for (const platform of PLATFORMS) {
    const entry = platforms[platform];
    if (!entry || typeof entry !== "object") {
      problems.push(`缺 ${platform}`);
      continue;
    }
    if (typeof entry.url !== "string") {
      problems.push(`${platform} 没有 url`);
    } else if (typeof version === "string" && SEMVER.test(version)) {
      problems.push(...checkUrl(platform, entry.url, version));
    }
    const fields = signedFields(entry.signature);
    if (fields === null) {
      problems.push(`${platform} 的签名格式不对（应当是 tauri signer sign 生成的 .sig 的内容）`);
      continue;
    }
    if (fields.version === undefined) {
      problems.push(`${platform} 的签名里没有版本号（签名时漏了 --app-version？应用会拒收）`);
    } else if (fields.version !== version) {
      problems.push(`${platform} 的签名绑的是 ${fields.version}，清单写的是 ${version}（应用会拒收）`);
    }
    if (typeof version === "string" && SEMVER.test(version) && fields.file !== assetName(version, platform)) {
      problems.push(`${platform} 的签名签的是 ${fields.file}，不是 ${assetName(version, platform)}（贴错了另一个包的签名？）`);
    }
  }
  for (const platform of Object.keys(platforms)) {
    if (!PLATFORMS.includes(platform)) problems.push(`多出不认识的平台 ${platform}`);
  }
  return problems;
}

/**
 * 用应用里写死的公钥，给清单里的每个安装包真的验一次签；验过的签名里的版本号、文件名还要和清单对上。
 * `readAsset(文件名)` 返回安装包内容（读不到就抛）。返回问题列表（空 = 全部验过）。
 */
export function verifyAssets(manifest, pubkey, readAsset) {
  const problems = [];
  if (!manifest || typeof manifest !== "object") return ["清单不是 JSON 对象"];
  for (const platform of PLATFORMS) {
    const entry = manifest.platforms?.[platform];
    if (!entry || typeof entry.url !== "string") {
      problems.push(`缺 ${platform}`);
      continue;
    }
    let name;
    try {
      name = decodeURIComponent(new URL(entry.url).pathname.split("/").pop() ?? "");
    } catch {
      problems.push(`${platform} 的地址不是合法的网址：${entry.url}`);
      continue;
    }
    let bytes;
    try {
      bytes = readAsset(name);
    } catch (err) {
      problems.push(`${platform} 读不到 ${name}：${err instanceof Error ? err.message : String(err)}`);
      continue;
    }
    const result = verifyMinisign(bytes, entry.signature, pubkey);
    if (!result.ok) {
      problems.push(`${platform}（${name}）验签没过：${result.reason}`);
      continue;
    }
    if (result.fields.version !== manifest.version) {
      problems.push(`${platform}（${name}）签名绑的是 ${result.fields.version}，清单写的是 ${manifest.version}（应用会拒收）`);
    }
    if (result.fields.file !== name) {
      problems.push(`${platform}（${name}）签名签的是 ${result.fields.file}，不是这个文件`);
    }
  }
  return problems;
}

// ---------- 命令行 ----------

/** `--asset 平台=路径` 可重复；其余 `--名字 值` 进 flags；不带 `--` 的进 positional。 */
export function parseArgs(argv) {
  const flags = {};
  const assets = {};
  const positional = [];
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--asset") {
      const [platform, ...rest] = (argv[(i += 1)] ?? "").split("=");
      assets[platform] = rest.join("=");
    } else if (arg.startsWith("--")) {
      flags[arg.slice(2)] = argv[(i += 1)];
    } else {
      positional.push(arg);
    }
  }
  return { flags, assets, positional };
}

function readManifest(source) {
  if (!source) throw new Error("缺清单文件（- 表示标准输入）");
  const text = readFileSync(source === "-" ? 0 : source, "utf8");
  try {
    return JSON.parse(text);
  } catch (err) {
    throw new Error(`清单不是合法 JSON：${err.message}`);
  }
}

function report(problems, what) {
  if (problems.length === 0) return false;
  console.error(`${what}有 ${problems.length} 个问题：`);
  for (const problem of problems) console.error(`  - ${problem}`);
  return true;
}

function main(argv) {
  const [command, ...rest] = argv;
  const { flags, assets, positional } = parseArgs(rest);

  if (command === "build") {
    if (!flags.version) throw new Error("缺 --version");
    if (!flags.out) throw new Error("缺 --out");
    const resolved = {};
    for (const [platform, path] of Object.entries(assets)) {
      resolved[platform] = {
        fileName: basename(path),
        signature: readFileSync(`${path}.sig`, "utf8"),
      };
    }
    const notes = flags["notes-file"] ? readFileSync(flags["notes-file"], "utf8") : "";
    const manifest = buildManifest({ version: flags.version, notes, assets: resolved });
    writeFileSync(flags.out, `${JSON.stringify(manifest, null, 2)}\n`);
    console.log(`已生成 ${flags.out}（v${manifest.version}，${Object.keys(manifest.platforms).join(" + ")}）`);
    return 0;
  }

  if (command === "check") {
    const manifest = readManifest(positional[0]);
    if (report(validateManifest(manifest, { expectVersion: flags.version }), "清单")) return 1;
    console.log(`清单没问题：v${manifest.version}，${Object.keys(manifest.platforms).join(" + ")}，地址、文件名、签名绑定的版本都对得上`);
    return 0;
  }

  if (command === "verify") {
    const manifest = readManifest(positional[0]);
    if (!flags.dir) throw new Error("缺 --dir（放着安装包的目录）");
    if (!flags.pubkey) throw new Error("缺 --pubkey（公钥，或 @公钥文件）");
    const pubkey = flags.pubkey.startsWith("@") ? readFileSync(flags.pubkey.slice(1), "utf8").trim() : flags.pubkey;
    const problems = [
      ...validateManifest(manifest, { expectVersion: flags.version }),
      ...verifyAssets(manifest, pubkey, (name) => readFileSync(join(flags.dir, basename(name)))),
    ];
    if (report(problems, "验签")) return 1;
    console.log(`验签通过：v${manifest.version}，${Object.keys(manifest.platforms).join(" + ")}，文件内容、版本号、文件名都被应用里的公钥确认过`);
    return 0;
  }

  console.error("用法：update-manifest.mjs build|check|verify …（见文件开头的说明）");
  return 2;
}

// 被 import（测试）时不跑命令行；直接执行时才跑
const invokedDirectly = (() => {
  try {
    return process.argv[1] !== undefined && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url);
  } catch {
    return false;
  }
})();
if (invokedDirectly) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (err) {
    console.error(err.message);
    process.exitCode = 1;
  }
}
