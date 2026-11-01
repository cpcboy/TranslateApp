#!/bin/bash
# 构建两种芯片（Apple 芯片 / Intel）的更新包，生成 latest.json；加 --publish 并确认后发布到 GitHub Release。
#
# 用法：
#   ./tools/release.sh                    只构建：产物放进 dist-release/，不上传任何东西
#   ./tools/release.sh --notes-file n.md  同上，更新说明取自 n.md（应用里会显示；相对路径按你当前所在的目录算）
#   ./tools/release.sh --publish          构建后，经你输入 yes 确认，创建 GitHub Release 并上传（必须在交互式终端里运行）
#
# 版本号取自 src-tauri/tauri.conf.json（Cargo.toml、Cargo.lock、package.json 必须与它一致）。
# 发布前先改版本号、提交，再 push（push 需要你另行授权）；加 --publish 时脚本一开始就核对 HEAD 已经在远端、
# 这个版本还没发布过，不会等构建了十分钟才发现。
#
# 需要：
#   - 更新私钥 ~/.tauri/sense-updater.key（TAURI_SIGNING_PRIVATE_KEY_PATH 可改），旁边有 .pub，文件权限 600，
#     而且不能放在仓库目录里（仓库是公开的）；
#     密码用环境变量 TAURI_SIGNING_PRIVATE_KEY_PASSWORD 给，没给就在运行时问（没设密码直接回车）
#   - 钥匙串里的代码签名身份 "SENSE Dev"（SENSE_SIGN_IDENTITY 可改）。升级前后签名身份必须不变，
#     屏幕录制授权和钥匙串里「始终允许」才不会在每次更新后掉光——所以找不到它就拒绝发布，不会悄悄退成 ad-hoc。
#     脚本按证书的 SHA-1 指纹认（SENSE_SIGN_IDENTITY_SHA1 可改），同名的另一张证书不算
#   - rustup target add aarch64-apple-darwin x86_64-apple-darwin；swiftc（Xcode 命令行工具）；python3；curl
#   - --publish 还要 gh 已登录
#
# 构建出来的东西会被逐项核对：芯片、sense-ocr 的最低系统版本、签名、包里没有 ._ 附属文件、主程序里确实带着这把公钥，
# 以及用应用里写死的公钥给两个包真的验一次签；发布之后再从线上把两个包下载回来重新验一遍。
#
# 更新私钥丢了或泄露了怎么办：见 sense/README.md「自动更新」一节里的「更换更新密钥、私钥丢了或泄露了」。
set -euo pipefail

# --notes-file 给的相对路径是相对用户当前目录的：先记下来，下面 cd 之后再解析
START_DIR="$PWD"
cd "$(dirname "$0")/.."

REPO="cpcboy/TranslateApp"
OUT="dist-release"
CONF="src-tauri/tauri.conf.json"
KEY_PATH="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.tauri/sense-updater.key}"
IDENTITY="${SENSE_SIGN_IDENTITY:-SENSE Dev}"
# 签名身份证书的 SHA-1 指纹：应用的「指定要求」里写的就是它。钥匙串里将来如果出现第二张同名证书，
# 按名字签会悄悄换成另一张，所有人的屏幕录制 / 钥匙串授权都会掉光——所以认指纹，不认名字
IDENTITY_SHA1="${SENSE_SIGN_IDENTITY_SHA1:-9F6C3A9CDDC20D25411694388609F1D87B3EA4F0}"
# 目标三元组 : 文件名里的芯片标签 : lipo 看到的芯片 : 更新清单里的平台键
TARGETS="aarch64-apple-darwin:aarch64:arm64:darwin-aarch64 x86_64-apple-darwin:x64:x86_64:darwin-x86_64"
# OCR 辅助程序允许要求的最低 macOS（与 build.rs 的 OCR_MIN_MACOS 一致）
OCR_MIN_MACOS="13.0"

fail() {
  echo "错误：$*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "找不到命令 $1"
}

# 字符串 $1 里有没有子串 $2。判断「某条命令的输出里有没有某个字」一律先把输出存进变量再用它：
# `cmd | grep -q` 在 set -o pipefail 下，grep 一匹配就退出、cmd 来不及写完被 SIGPIPE，整条管道会莫名其妙变成失败
contains() {
  case "$1" in
    *"$2"*) return 0 ;;
    *) return 1 ;;
  esac
}

PUBLISH=0
NOTES_FILE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --publish) PUBLISH=1 ;;
    --notes-file)
      [ $# -ge 2 ] || fail "--notes-file 后面要跟文件"
      NOTES_FILE="$2"
      shift
      ;;
    -h | --help)
      sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) fail "不认识的参数：$1" ;;
  esac
  shift
done

if [ -n "$NOTES_FILE" ]; then
  case "$NOTES_FILE" in
    /*) ;;
    *) NOTES_FILE="$START_DIR/$NOTES_FILE" ;;
  esac
  [ -f "$NOTES_FILE" ] || fail "找不到更新说明文件 ${NOTES_FILE}"
fi

# 仅供本机演练（端到端验证更新流程）的三个开关：临时换掉 tauri 配置（测试公钥 / 本机地址 / 假版本号）、
# 允许工作区有未提交的改动、跳过测试。用了它们产出的包绝不能进 Release，所以和 --publish 互斥。
TEST_CONFIG="${SENSE_RELEASE_TEST_CONFIG:-}"
ALLOW_DIRTY="${SENSE_RELEASE_ALLOW_DIRTY:-}"
SKIP_TESTS="${SENSE_RELEASE_SKIP_TESTS:-}"
if [ "$PUBLISH" = 1 ] && { [ -n "$TEST_CONFIG" ] || [ -n "$ALLOW_DIRTY" ] || [ -n "$SKIP_TESTS" ]; }; then
  fail "SENSE_RELEASE_TEST_CONFIG / _ALLOW_DIRTY / _SKIP_TESTS 只用于本机演练，不能和 --publish 一起用"
fi

need node
need npm
need rustup
need swiftc
need codesign
need lipo
need vtool
need tar
need shasum
need python3
need curl
if [ "$PUBLISH" = 1 ]; then need gh; fi

# 这些环境变量会悄悄改变构建结果（换配置、抬高系统最低版本、跳过 OCR 辅助程序、改产物目录、加编译选项）：发布构建不受它们影响
unset TAURI_CONFIG MACOSX_DEPLOYMENT_TARGET SENSE_SKIP_OCR CARGO_TARGET_DIR CARGO_BUILD_TARGET CARGO_BUILD_TARGET_DIR RUSTFLAGS CARGO_ENCODED_RUSTFLAGS

# 临时目录统一放在这里，不管从哪里退出都清掉
WORK_DIR="$(mktemp -d)"
trap 'rm -rf -- "${WORK_DIR:?}"' EXIT

# 读 JSON 里的一个字段：json_get <文件|-> <a.b.c>，- 表示标准输入。字段不存在输出空串。
json_get() {
  node -e '
    const fs = require("fs");
    let value = JSON.parse(fs.readFileSync(process.argv[1] === "-" ? 0 : process.argv[1], "utf8"));
    for (const key of process.argv[2].split(".")) value = value == null ? undefined : value[key];
    process.stdout.write(value === undefined ? "" : String(value));
  ' "$1" "$2"
}

# 生效的 tauri 配置里的一个字段：演练时以 SENSE_RELEASE_TEST_CONFIG 里的为准
effective_conf() {
  local value=""
  if [ -n "$TEST_CONFIG" ]; then value="$(printf '%s' "$TEST_CONFIG" | json_get - "$1")"; fi
  if [ -z "$value" ]; then value="$(json_get "$CONF" "$1")"; fi
  printf '%s' "$value"
}

# ---------- 版本 ----------
VERSION="$(json_get "$CONF" version)"
if [ -n "$TEST_CONFIG" ]; then
  OVERRIDE="$(printf '%s' "$TEST_CONFIG" | json_get - version)"
  if [ -n "$OVERRIDE" ]; then VERSION="$OVERRIDE"; fi
  echo "!! 演练模式：用测试配置覆盖了 tauri 配置（版本 ${VERSION}）。产物不能发布"
else
  PKG_VERSION="$(json_get package.json version)"
  CARGO_VERSION="$(awk -F'"' '/^version = / { print $2; exit }' src-tauri/Cargo.toml)"
  # Cargo.lock 里 sense 自己那一条：改了 Cargo.toml 的版本却没让它跟着变，--locked 的构建会直接失败
  LOCK_VERSION="$(awk '/^name = "sense"$/ { found = 1; next } found && /^version = / { gsub(/"/, "", $3); print $3; exit }' src-tauri/Cargo.lock)"
  [ "$VERSION" = "$PKG_VERSION" ] || fail "版本不一致：tauri.conf.json 是 ${VERSION}，package.json 是 ${PKG_VERSION}"
  [ "$VERSION" = "$CARGO_VERSION" ] || fail "版本不一致：tauri.conf.json 是 ${VERSION}，Cargo.toml 是 ${CARGO_VERSION}"
  [ "$VERSION" = "$LOCK_VERSION" ] ||
    fail "版本不一致：tauri.conf.json 是 ${VERSION}，Cargo.lock 里 sense 是 ${LOCK_VERSION:-（读不到）}。在 src-tauri 里跑一次 cargo check 让它跟着更新，再一起提交"

  # 更新相关的安全开关：发出去的包必须带着这些
  [ "$(json_get "$CONF" plugins.updater.requireSignedVersion)" = "true" ] ||
    fail "tauri.conf.json 里 plugins.updater.requireSignedVersion 必须是 true（它防止有人把旧版本的包冒充成新版本，让装机的人降级）"
  case "$(json_get "$CONF" plugins.updater.dangerousInsecureTransportProtocol)" in
    "" | false) ;;
    *) fail "tauri.conf.json 里 plugins.updater.dangerousInsecureTransportProtocol 不能打开：它允许走明文 http 下载更新" ;;
  esac
  case "$(json_get "$CONF" plugins.updater.endpoints.0)" in
    "https://github.com/${REPO}/releases/latest/download/"*) ;;
    *) fail "tauri.conf.json 里 plugins.updater.endpoints 的第一个地址不是 https://github.com/${REPO}/releases/latest/download/ 开头的" ;;
  esac
fi
grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' <<<"$VERSION" || fail "版本号必须是 x.y.z：${VERSION}"
echo "==> 发布版本 v${VERSION}"

# ---------- 前提 ----------
START_HEAD="$(git rev-parse HEAD)"

# 构建会生成 dist/、target/ 这些（都被 .gitignore 了）；如果它还改了被跟踪的文件，说明发出去的包不是这个提交的样子
assert_clean_tree() {
  if [ -n "$ALLOW_DIRTY" ]; then return 0; fi
  [ "$(git rev-parse HEAD)" = "$START_HEAD" ] || fail "构建期间 HEAD 变了（原来是 ${START_HEAD}）：发布出去的构建必须来自一个确定的提交"
  if [ -n "$(git status --porcelain)" ]; then
    git status --short >&2
    fail "工作区有没提交的改动（含新文件，构建本身改出来的也算）。发布出去的构建必须能从某个提交复现：先提交"
  fi
}

if [ -n "$ALLOW_DIRTY" ]; then
  echo "!! 演练模式：不检查工作区是否干净"
else
  assert_clean_tree
fi

if [ "$PUBLISH" = 1 ]; then
  # 提示符处的 yes 不是安全边界（管道一样喂得进去），但至少不让无人值守的脚本 / 代理悄悄发布：
  # 必须在交互式终端里亲自输入。创建 Release 每次都需要负责人当次授权
  [ -t 0 ] || fail "--publish 必须在交互式终端里运行：要在提示符处亲自输入 yes（创建 Release 每次都需要负责人当次授权）"
  # 发布的前提放在构建之前查：别等构建了十分钟才发现没 push、版本已经发布过了
  ORIGIN_URL="$(git remote get-url origin 2>/dev/null || true)"
  case "$ORIGIN_URL" in
    "https://github.com/${REPO}" | "https://github.com/${REPO}.git" | "git@github.com:${REPO}" | "git@github.com:${REPO}.git") ;;
    *) fail "origin 指向 ${ORIGIN_URL:-（没有）}，不是 github.com/${REPO}：Release 会发到别的仓库" ;;
  esac
  gh auth status >/dev/null 2>&1 || fail "gh 还没登录：gh auth login"
  git fetch --quiet origin main
  git merge-base --is-ancestor HEAD origin/main ||
    fail "HEAD（$(git rev-parse --short HEAD)）还不在 origin/main 上：先 push（push 需要你单独授权），否则 Release 的标签会指向远端没有的提交"
  if gh release view "v${VERSION}" --repo "$REPO" >/dev/null 2>&1; then
    fail "v${VERSION} 已经发布过了。改版本号再来；已发布的版本不要覆盖（装机的人可能已经下了）"
  fi
fi

INSTALLED_TARGETS="$(rustup target list --installed)"
for TRIPLE in aarch64-apple-darwin x86_64-apple-darwin; do
  grep -qx "$TRIPLE" <<<"$INSTALLED_TARGETS" || fail "缺 Rust 目标 ${TRIPLE}：rustup target add ${TRIPLE}"
done

# 钥匙串里要有「指纹和名字都对得上」的那一行，如 `  1) 9F6C…F0 "SENSE Dev"`
IDENTITIES="$(security find-identity -p codesigning 2>/dev/null || true)"
IDENTITY_FOUND=0
while IFS= read -r LINE; do
  if contains "$LINE" "$IDENTITY_SHA1" && contains "$LINE" "\"${IDENTITY}\""; then IDENTITY_FOUND=1; fi
done <<<"$IDENTITIES"
[ "$IDENTITY_FOUND" = 1 ] ||
  fail "钥匙串里没有指纹是 ${IDENTITY_SHA1} 的代码签名身份 \"${IDENTITY}\"。没有它就只能 ad-hoc 签名（或换成另一张证书），每次更新后屏幕录制、钥匙串授权都会掉光——不发布"
IDENTITY_SHA1_LOWER="$(printf '%s' "$IDENTITY_SHA1" | tr 'A-F' 'a-f')"

[ -f "$KEY_PATH" ] || fail "找不到更新私钥 ${KEY_PATH}（生成办法见 sense/README.md「自动更新」）"
[ -f "$KEY_PATH.pub" ] || fail "找不到 ${KEY_PATH}.pub（丢了可以把 tauri.conf.json 里的 plugins.updater.pubkey 原样写成这个文件）"
# 私钥不能放在仓库目录里：仓库是公开的，一次 git add -A 就是不可撤销的泄露（sense/.gitignore 只是第二道）
KEY_DIR="$(cd "$(dirname "$KEY_PATH")" && pwd -P)"
REPO_ROOT="$(git rev-parse --show-toplevel)"
case "${KEY_DIR}/" in
  "${REPO_ROOT}"/*) fail "更新私钥 ${KEY_PATH} 放在仓库目录里（${REPO_ROOT}）：仓库是公开的。放到仓库外面，比如 ~/.tauri/" ;;
esac
case "$(stat -f %Lp "$KEY_PATH")" in
  600 | 400) ;;
  *) fail "更新私钥 ${KEY_PATH} 的权限是 $(stat -f %Lp "$KEY_PATH")，同组或其他人也读得到：chmod 600 '${KEY_PATH}'" ;;
esac
EXPECT_PUB="$(tr -d '\n' <"$KEY_PATH.pub")"
CONFIG_PUB="$(effective_conf plugins.updater.pubkey)"
[ -n "$CONFIG_PUB" ] || fail "tauri.conf.json 里没有 plugins.updater.pubkey"
# 公钥对不上 = 发出去的包装机之后永远验不过签名，而且装机的人看不出来。这里挡住
[ "$EXPECT_PUB" = "$CONFIG_PUB" ] ||
  fail "tauri.conf.json 里的 plugins.updater.pubkey 和 ${KEY_PATH}.pub 对不上。已装机器信的是前者，签名用的是后者"

# 私钥密码：环境变量给了就用（设成空 = 没密码），没给就问
if [ -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD+x}" ]; then
  if [ -t 0 ]; then
    read -r -s -p "更新私钥的密码（没设密码就直接回车）：" TAURI_SIGNING_PRIVATE_KEY_PASSWORD
    echo
  else
    fail "不是交互终端：请用环境变量 TAURI_SIGNING_PRIVATE_KEY_PASSWORD 给私钥密码（没设密码就设成空）"
  fi
fi
SIGN_PASSWORD="$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
unset TAURI_SIGNING_PRIVATE_KEY_PASSWORD TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PATH

# ---------- 测试 ----------
if [ -n "$SKIP_TESTS" ]; then
  echo "!! 演练模式：跳过测试"
else
  echo "==> 先跑测试（不绿不发布）"
  npm run typecheck
  npm test
  (cd src-tauri && cargo test --locked)
fi

# ---------- 构建 + 签名 + 打包 ----------
mkdir -p "$OUT"
# 上一次留下的包不能混进这一次
rm -f -- "$OUT"/SENSE_*.app.tar.gz "$OUT"/SENSE_*.app.tar.gz.sig "$OUT"/latest.json

for SPEC in $TARGETS; do
  IFS=: read -r TRIPLE LABEL LIPO_ARCH PLATFORM <<<"$SPEC"
  echo "==> [${PLATFORM}] 构建 ${TRIPLE}"
  # --locked：只按 Cargo.lock 里写定的依赖版本构建，不会在构建时悄悄升级依赖
  if [ -n "$TEST_CONFIG" ]; then
    npm run tauri build -- --target "$TRIPLE" --config "$TEST_CONFIG" -- --locked
  else
    npm run tauri build -- --target "$TRIPLE" -- --locked
  fi
  assert_clean_tree

  APP="src-tauri/target/$TRIPLE/release/bundle/macos/SENSE.app"
  [ -d "$APP" ] || fail "没找到 ${APP}，构建失败了？"

  # 芯片对不对、OCR 辅助程序的最低系统版本是不是被抬高了——后者发出去的包在旧系统上截屏翻译会直接起不来
  for BIN in sense sense-ocr; do
    ARCHS="$(lipo -archs "$APP/Contents/MacOS/$BIN")"
    [ "$ARCHS" = "$LIPO_ARCH" ] || fail "${APP} 里的 ${BIN} 是 ${ARCHS}，应当是 ${LIPO_ARCH}"
  done
  VTOOL_OUT="$(vtool -show-build "$APP/Contents/MacOS/sense-ocr")"
  MINOS="$(awk '/minos/ { print $2; exit }' <<<"$VTOOL_OUT")"
  # 读不出来就是失败，不能当「没问题」放过去
  case "$MINOS" in
    [0-9]*.[0-9]* | [0-9]*.[0-9]*.[0-9]*) ;;
    *) fail "读不出 sense-ocr 的最低系统版本（vtool 给的是「${MINOS}」）" ;;
  esac
  awk -v v="$MINOS" -v max="$OCR_MIN_MACOS" 'BEGIN {
    split(v, a, "."); split(max, b, ".")
    exit !((a[1] + 0 < b[1] + 0) || (a[1] + 0 == b[1] + 0 && a[2] + 0 <= b[2] + 0))
  }' ||
    fail "sense-ocr 要求 macOS ${MINOS}，高于 ${OCR_MIN_MACOS}：旧系统上截屏翻译会起不来（build.rs 的 -target 没生效？删掉 src-tauri/binaries 再来）"
  echo "    芯片 ${LIPO_ARCH}，sense-ocr 最低 macOS ${MINOS}"

  # 二进制里确实带着配置里这把公钥：公钥错了，装机的人永远收不到更新，而且谁都看不出来。
  # 只查公钥（152 个字符，两种芯片的二进制里都原样躺在只读数据里）。更新地址查不了：x86_64 的编译器会把
  # 几十个字节以内的字符串字面量拆成立即数写进代码（实测 "http://127.0.0.1:18765/latest.json" 被拆成 5 段），搜不到整串
  grep -aqF -- "$CONFIG_PUB" "$APP/Contents/MacOS/sense" ||
    fail "${APP} 的主程序里找不到 tauri.conf.json 里的更新公钥：构建没带上这份配置？（如果是编译器把它拆开了，需要换个办法查，别直接去掉这一步）"

  # 与 install-app.sh 同一种签法（已在本机验证）。签名在先、打包在后：包里的 .app 就是最终签好名的那份
  echo "==> [${PLATFORM}] 用 \"${IDENTITY}\"（${IDENTITY_SHA1}）签名"
  codesign --force --deep --sign "$IDENTITY_SHA1" "$APP"
  codesign --verify --deep --strict "$APP"
  # 指定要求里写的证书指纹就是钥匙串授权认的东西：必须是上面钉住的那一张
  REQUIREMENT="$(codesign -d -r- "$APP" 2>&1)"
  contains "$REQUIREMENT" "certificate leaf = H\"${IDENTITY_SHA1_LOWER}\"" ||
    fail "${APP} 签完之后的指定要求里没有证书指纹 ${IDENTITY_SHA1_LOWER}：屏幕录制 / 钥匙串授权认不出它是同一个应用"
  codesign -dv "$APP" 2>&1 | grep -E "Identifier|Authority|Signature" | sed 's/^/    /' || true

  ASSET="SENSE_${VERSION}_${LABEL}.app.tar.gz"
  echo "==> [${PLATFORM}] 打包 ${ASSET}"
  # COPYFILE_DISABLE / --no-xattrs：不让 macOS 往包里塞 ._ 附属文件和扩展属性
  COPYFILE_DISABLE=1 tar --no-xattrs -czf "$OUT/$ASSET" -C "$(dirname "$APP")" SENSE.app
  # 用 python 的 tarfile 看原始成员名：bsdtar 列包时会把 ._ 附属文件合并掉，grep 不到它们
  python3 -I - "$OUT/$ASSET" <<'PY' || fail "${ASSET} 的内容不合格（见上）"
import sys
import tarfile

problems = []
with tarfile.open(sys.argv[1], "r:gz") as tar:
    members = tar.getmembers()
    names = {member.name for member in members}
    for member in members:
        parts = member.name.split("/")
        if member.name.startswith("/") or ".." in parts:
            problems.append(f"危险的路径：{member.name}")
        if parts[0] != "SENSE.app":
            problems.append(f"顶层不是 SENSE.app（更新插件按这个结构解包）：{member.name}")
        if parts[-1].startswith("._"):
            problems.append(f"混进了 macOS 的 ._ 附属文件：{member.name}")
        if (member.issym() or member.islnk()) and (member.linkname.startswith("/") or ".." in member.linkname.split("/")):
            problems.append(f"链接指到了包外面：{member.name} -> {member.linkname}")
    for required in ("SENSE.app/Contents/MacOS/sense", "SENSE.app/Contents/MacOS/sense-ocr", "SENSE.app/Contents/Info.plist"):
        if required not in names:
            problems.append(f"缺 {required}")
for problem in problems:
    print(f"    {problem}", file=sys.stderr)
sys.exit(1 if problems else 0)
PY

  # 解包回来再验一次签名：证明发出去的是一个完好的、签过名的 .app，而不只是构建目录里那份
  CHECK_DIR="$WORK_DIR/check-${LABEL}"
  mkdir -p "$CHECK_DIR"
  tar -xzf "$OUT/$ASSET" -C "$CHECK_DIR"
  codesign --verify --deep --strict "$CHECK_DIR/SENSE.app" || fail "${ASSET} 解包后的 .app 签名失效"
  rm -rf -- "${CHECK_DIR:?}"

  echo "==> [${PLATFORM}] 用更新私钥签名（绑定版本 ${VERSION}）"
  if [ -n "$SIGN_PASSWORD" ]; then
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$SIGN_PASSWORD" npx --no-install tauri signer sign --app-version "$VERSION" -f "$KEY_PATH" "$OUT/$ASSET" >/dev/null
  else
    npx --no-install tauri signer sign --app-version "$VERSION" -f "$KEY_PATH" -p "" "$OUT/$ASSET" >/dev/null
  fi
  [ -s "$OUT/$ASSET.sig" ] || fail "没生成 ${OUT}/${ASSET}.sig"
done

# ---------- 清单 ----------
echo "==> 生成并核对 latest.json"
if [ -n "$NOTES_FILE" ]; then
  node tools/update-manifest.mjs build --version "$VERSION" --notes-file "$NOTES_FILE" --out "$OUT/latest.json" \
    --asset "darwin-aarch64=$OUT/SENSE_${VERSION}_aarch64.app.tar.gz" --asset "darwin-x86_64=$OUT/SENSE_${VERSION}_x64.app.tar.gz"
else
  node tools/update-manifest.mjs build --version "$VERSION" --out "$OUT/latest.json" \
    --asset "darwin-aarch64=$OUT/SENSE_${VERSION}_aarch64.app.tar.gz" --asset "darwin-x86_64=$OUT/SENSE_${VERSION}_x64.app.tar.gz"
fi
node tools/update-manifest.mjs check "$OUT/latest.json" --version "$VERSION"

# 用应用里写死的公钥（不是 .pub 文件，虽然两者已经比对过一致）给两个包真的验一次签：
# 通过 = 装了这个版本的电脑也会放行它
echo "==> 用应用里的公钥给两个包验签"
node tools/update-manifest.mjs verify "$OUT/latest.json" --dir "$OUT" --pubkey "$CONFIG_PUB" --version "$VERSION"

FILES="SENSE_${VERSION}_aarch64.app.tar.gz SENSE_${VERSION}_aarch64.app.tar.gz.sig SENSE_${VERSION}_x64.app.tar.gz SENSE_${VERSION}_x64.app.tar.gz.sig latest.json"
echo "==> 产物（${OUT}/）"
# shellcheck disable=SC2086  # 文件名都没有空格，按词拆分是有意的
(cd "$OUT" && ls -l $FILES && shasum -a 256 SENSE_"${VERSION}"_*.app.tar.gz)

if [ "$PUBLISH" != 1 ]; then
  echo
  echo "只构建了，没有上传任何东西。要发布：先提交并 push，再 ./tools/release.sh --publish"
  exit 0
fi

# ---------- 发布 ----------
assert_clean_tree
# 前面的前提查过一次，构建这么久之后再查一次（这个版本别被别处抢先发了）
if gh release view "v${VERSION}" --repo "$REPO" >/dev/null 2>&1; then
  fail "v${VERSION} 在这期间被发布了。改版本号再来；已发布的版本不要覆盖"
fi

echo
echo "即将在 github.com/${REPO} 发布 v${VERSION}（提交 $(git rev-parse --short HEAD)），上传："
echo "  ${FILES}"
echo "发布后，装了带自动更新版本的电脑会在 6 小时内自动发现并下载它。"
# 标准输入被关掉（EOF）时 read 会失败：不能让 set -e 悄悄把脚本带走，要说清楚是「没有确认」
read -r -p "输入 yes 确认发布：" ANSWER || ANSWER=""
[ "$ANSWER" = "yes" ] || fail "没有确认，已取消（构建产物还在 ${OUT}/）"
# 提示符前停了多久不知道：这期间提交了新东西、改了文件的话，现在的 HEAD 就不是刚才构建、验签的那一个了
assert_clean_tree

ARGS=""
for FILE in $FILES; do ARGS="$ARGS $OUT/$FILE"; done
# 标签指向构建时的那个提交（START_HEAD），不是「此刻的 HEAD」
if [ -n "$NOTES_FILE" ]; then
  # shellcheck disable=SC2086
  gh release create "v${VERSION}" $ARGS --repo "$REPO" --target "$START_HEAD" --title "SENSE v${VERSION}" --notes-file "$NOTES_FILE" || RELEASE_FAILED=1
else
  # shellcheck disable=SC2086
  gh release create "v${VERSION}" $ARGS --repo "$REPO" --target "$START_HEAD" --title "SENSE v${VERSION}" --notes "SENSE v${VERSION}" || RELEASE_FAILED=1
fi
if [ "${RELEASE_FAILED:-0}" = 1 ]; then
  # 上传到一半断了，GitHub 上可能留下一个草稿或半个 Release：先看清楚再重来，别直接再跑一遍
  echo "gh release create 失败了。先去 https://github.com/${REPO}/releases 看有没有留下草稿或半个 v${VERSION}；有的话先撤掉再重发：" >&2
  echo "  gh release delete v${VERSION} --repo ${REPO} --cleanup-tag --yes" >&2
  exit 1
fi

# 发布后核对线上：应用读的就是这个网址。CDN 偶尔要几秒才反映，重试几次。
# 光看清单不够——把清单里指向的两个包也真的下载回来，用应用里的公钥再验一遍，这才是用户会拿到的东西
echo "==> 核对线上的 latest.json 和两个安装包（应用读的网址）"
LIVE_DIR="$WORK_DIR/live"
for ATTEMPT in 1 2 3 4 5; do
  rm -rf -- "${LIVE_DIR:?}"
  mkdir -p "$LIVE_DIR"
  if curl -fsSL -o "$LIVE_DIR/latest.json" "https://github.com/${REPO}/releases/latest/download/latest.json" &&
    node tools/update-manifest.mjs check "$LIVE_DIR/latest.json" --version "$VERSION" &&
    curl -fsSL -o "$LIVE_DIR/SENSE_${VERSION}_aarch64.app.tar.gz" "$(json_get "$LIVE_DIR/latest.json" platforms.darwin-aarch64.url)" &&
    curl -fsSL -o "$LIVE_DIR/SENSE_${VERSION}_x64.app.tar.gz" "$(json_get "$LIVE_DIR/latest.json" platforms.darwin-x86_64.url)" &&
    node tools/update-manifest.mjs verify "$LIVE_DIR/latest.json" --dir "$LIVE_DIR" --pubkey "$CONFIG_PUB" --version "$VERSION"; then
    echo "完成。线上的清单和两个安装包都核对过了。"
    exit 0
  fi
  echo "第 ${ATTEMPT} 次没对上，3 秒后重试…"
  sleep 3
done
echo "Release 已经公开了，但线上的清单或安装包一直核对不过。装了带自动更新版本的电脑会拒绝验不过签的包（不会装坏），但会一直提示更新失败。" >&2
echo "先看看：https://github.com/${REPO}/releases（可能被标成了预发布 / 草稿，latest 不会指向它）。要撤下：" >&2
echo "  gh release delete v${VERSION} --repo ${REPO} --cleanup-tag --yes" >&2
exit 1
