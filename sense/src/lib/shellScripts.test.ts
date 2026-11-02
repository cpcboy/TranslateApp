import { describe, expect, it } from "vitest";
// 用 Vite 的 ?raw 读脚本源码：不引入 Node 类型依赖（同 mockProvider.test.ts）
import releaseSh from "../../tools/release.sh?raw";
import gitignore from "../../.gitignore?raw";

// 用 glob 而不是手写清单：以后在 tools/ 里新加的 .sh 自动被扫到
const scripts = import.meta.glob("../../tools/*.sh", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/**
 * macOS 自带的 bash 是 3.2：变量名后面紧跟中文（`"版本 $VERSION，…"`）时，在没设语言环境的场景里
 * 它会把中文的字节也当成变量名的一部分，`set -u` 下直接报 `unbound variable`——这个仓库已经踩过两次。
 * 最阴险的是它常常藏在「只有失败时才执行」的报错字符串里：平时跑得好好的，出事那天用户看到的
 * 不是写好的原因，而是一行 unbound variable。所以变量后面紧跟非 ASCII 字符必须写成 `${VERSION}`。
 */
const bareVariableBeforeNonAscii = /\$[A-Za-z_][A-Za-z_0-9]*(?=[^\x00-\x7F])/g;

describe("shell 脚本：变量名后面不能直接跟中文", () => {
  it("扫到了 tools/ 里的全部脚本（至少有 install-app.sh 和 release.sh）", () => {
    const names = Object.keys(scripts).map((path) => path.split("/").pop());
    expect(names).toContain("install-app.sh");
    expect(names).toContain("release.sh");
  });

  for (const [path, source] of Object.entries(scripts)) {
    it(`${path.replace("../../", "")} 里没有 $变量 紧跟非 ASCII 字符的写法`, () => {
      const hits = source
        .split("\n")
        .map((line, index) => ({ line, number: index + 1 }))
        .flatMap(({ line, number }) => (line.match(bareVariableBeforeNonAscii) ?? []).map((hit) => `第 ${number} 行 ${hit}：${line.trim()}`));
      expect(hits).toEqual([]);
    });
  }

  it("检测本身是灵的：能抓到坏写法，放过 ${VAR} 和 $(...) 的写法", () => {
    expect("echo \"版本 $VERSION，好\"".match(bareVariableBeforeNonAscii)).toEqual(["$VERSION"]);
    expect("echo \"$A：$B）\"".match(bareVariableBeforeNonAscii)).toEqual(["$A", "$B"]);
    expect("echo \"版本 ${VERSION}，好 $(git rev-parse HEAD)）\"".match(bareVariableBeforeNonAscii)).toBeNull();
    expect("echo \"$VERSION ok $1，\"".match(bareVariableBeforeNonAscii)).toBeNull();
  });
});

/** 去掉整行注释，免得说明文字里提到 `--locked` 之类的词让下面的断言虚假通过 */
const code = releaseSh
  .split("\n")
  .filter((line) => !line.trim().startsWith("#"))
  .join("\n");

describe("release.sh：发布前的把关不能被改没了", () => {
  it("测试和构建都带 --locked（只按 Cargo.lock 里写定的依赖版本构建）", () => {
    expect(code).toContain("cargo test --locked");
    expect(code.match(/tauri build -- --target "\$TRIPLE"[^\n]*-- --locked/g)).toHaveLength(2); // 正式 / 演练配置各一处
  });

  it("每个包构建出来之后，用应用里写死的公钥真的验一次签；发布后把线上的包下载回来再验一次", () => {
    expect(code).toContain('update-manifest.mjs verify "$OUT/latest.json" --dir "$OUT" --pubkey "$CONFIG_PUB"');
    expect(code).toContain('update-manifest.mjs verify "$LIVE_DIR/latest.json" --dir "$LIVE_DIR" --pubkey "$CONFIG_PUB"');
  });

  it("--publish 的前提（origin、gh 登录、已 push、没发过）在第一次构建之前查，不等构建了十分钟才发现", () => {
    const build = code.indexOf("npm run tauri build");
    for (const guard of ["git remote get-url origin", "gh auth status", "git merge-base --is-ancestor HEAD origin/main", "gh release view"]) {
      const at = code.indexOf(guard);
      expect(at, guard).toBeGreaterThan(-1);
      expect(at, `${guard} 应当在构建之前`).toBeLessThan(build);
    }
  });

  it("包里的 ._ 附属文件用 python 的 tarfile 查（bsdtar 的 tar -t 会把它们合并掉，grep 不到）", () => {
    expect(code).toContain("tarfile.open");
    expect(code).not.toMatch(/tar -tzf[^\n]*grep/);
  });

  it("管道里不用 grep -q：pipefail 下 grep 一匹配就退出，前面的命令被 SIGPIPE，整条管道会莫名其妙变成失败", () => {
    const hits = code.split("\n").filter((line) => /\|\s*grep\s+-[A-Za-z]*q/.test(line));
    expect(hits).toEqual([]);
  });

  it("会悄悄改变构建结果的环境变量被清掉；签名身份按指纹钉住", () => {
    for (const name of ["TAURI_CONFIG", "MACOSX_DEPLOYMENT_TARGET", "SENSE_SKIP_OCR", "CARGO_TARGET_DIR", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]) {
      expect(code, name).toMatch(new RegExp(`unset [^\\n]*${name}`));
    }
    expect(code).toContain("9F6C3A9CDDC20D25411694388609F1D87B3EA4F0");
    expect(code).toContain('codesign --force --deep --sign "$IDENTITY_SHA1"');
  });

  it("Release 的标签钉在构建时的提交（START_HEAD），确认之后再核对一次工作区：提示符前停多久都不会把别的提交打上标签", () => {
    expect(code).toContain('--target "$START_HEAD"');
    expect(code).not.toContain("--target \"$(git rev-parse HEAD)\"");
    const confirm = code.indexOf('read -r -p "输入 yes 确认发布：');
    const lastCheck = code.lastIndexOf("assert_clean_tree");
    const create = code.indexOf("gh release create");
    expect(confirm).toBeGreaterThan(-1);
    expect(lastCheck, "确认之后、gh release create 之前要再核对一次").toBeGreaterThan(confirm);
    expect(lastCheck).toBeLessThan(create);
  });

  it("gh release create 失败要说清楚：可能留下草稿或半个 Release，先看再重来", () => {
    expect(code).toContain("gh release create 失败了");
  });

  it("不用会提前退出的管道（| head）：pipefail 下前一个命令可能被 SIGPIPE", () => {
    expect(code.split("\n").filter((line) => /\|\s*head\b/.test(line))).toEqual([]);
  });

  it("sense-ocr 的最低系统版本读不出来就是失败（不是放行）", () => {
    expect(code).toContain("读不出 sense-ocr 的最低系统版本");
  });

  it("确认发布的那个 read 在标准输入被关掉时也要走到「没有确认，已取消」，不能让 set -e 悄悄把脚本带走", () => {
    expect(code).toContain('read -r -p "输入 yes 确认发布：" ANSWER || ANSWER=""');
    expect(code).toContain("没有确认，已取消");
  });

  it("--publish 必须在交互式终端里运行，而且这一关在发布前提和第一次构建之前（管道喂进去的 yes 不能让无人值守的脚本发布）", () => {
    const tty = code.indexOf('[ -t 0 ] || fail "--publish ');
    expect(tty).toBeGreaterThan(-1);
    expect(tty).toBeLessThan(code.indexOf("git remote get-url origin"));
    expect(tty).toBeLessThan(code.indexOf("npm run tauri build"));
  });

  it("更新私钥不能放在仓库目录里（仓库是公开的），.gitignore 也兜着 *.key", () => {
    expect(code).toContain('REPO_ROOT="$(git rev-parse --show-toplevel)"');
    expect(code).toMatch(/case "\$\{KEY_DIR\}\/" in\n\s*"\$\{REPO_ROOT\}"\/\*\) fail /);
    const ignored = gitignore.split("\n");
    expect(ignored).toContain("*.key");
    expect(ignored).toContain("sense-updater*");
  });
});
