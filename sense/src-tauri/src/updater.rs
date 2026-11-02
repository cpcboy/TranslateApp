//! 自动更新。
//!
//! 流程（全部在 Rust 里，前端只收状态、点按钮）：
//! 启动 30 秒后检查一次，之后每 6 小时一次（失败 30 分钟后重试）→ 发现新版本就在后台下载并验签（minisign）→
//! 下载好的安装包留在内存里 → 用户点「立即重启」才替换 `.app` 并重启。
//!
//! **为什么不在后台悄悄重启**：窗口启动即显示（tauri.conf.json 的 `visible: true`），后台重启会把窗口弹到
//! 用户正在打字的地方抢走焦点；失焦会隐藏窗口，而长文翻译在窗口藏着的时候还在跑，重启就丢了。所以装的时机交给用户。
//!
//! **为什么不用插件自带的前端接口**：前端不碰网络，也不握签名材料。capabilities 里没有 `updater:*`，
//! 插件的命令因此从 WebView 调不到；前端只能走下面四个命令（且只认主窗口）。
//!
//! 更新源是 GitHub Release 上的 `latest.json`（端点、公钥、`requireSignedVersion` 见 tauri.conf.json 的
//! `plugins.updater`）。安装包的签名把版本号绑在里面，清单被篡改成「新版本号 + 旧安装包」会被拒。

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::{settings, window};

/// 前端据此刷新：载荷是完整的 `UpdateStatus`。
pub const EVENT: &str = "sense://update-status";

/// 启动后等这么久再做第一次检查：别和启动预热抢网络，也让刚装好重启的那次先稳住。
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);
/// 轮询间隔：每分钟看一眼「到点没有」，见 `due`。
const TICK: Duration = Duration::from_secs(60);
const CHECK_INTERVAL_MS: u64 = 6 * 60 * 60 * 1000;
const RETRY_AFTER_FAILURE_MS: u64 = 30 * 60 * 1000;
/// 取清单只是一个小 JSON，20 秒不通就当失败。
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// 安装包 5–6 MB；网络很慢（杭州直连 GitHub）时给足时间，15 分钟还没下完再放弃。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// 下载进度最多这么久推一次给前端。
const PROGRESS_EVERY: Duration = Duration::from_millis(300);
/// 更新说明留给界面的字数上限：清单里的 `notes` 不受我们控制，别让它撑爆界面。
const NOTES_MAX_CHARS: usize = 600;
/// 设置文件里的开关：缺省 = 开。
const AUTO_UPDATE_KEY: &str = "auto_update";
/// 设置文件里的「最近一次更新」记录（装好、重启之前写下，重启后在设置页能看到）。
const LAST_UPDATE_KEY: &str = "last_update";

/// 当前正在做什么，给界面看的。`Ready` 优先于 `Checking`：已经下载好的版本在后台复查时横幅不能闪掉。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Phase {
    /// 还没检查过
    Idle,
    Checking,
    UpToDate,
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    /// 已下载、验过签，等用户点「立即重启」
    Ready { version: String },
    Installing { version: String },
}

/// 失败是哪一类：决定界面怎么说、会不会自动重试。
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// 检查 / 下载 / 验签没成：到点会自动重试（自动检查开着的话）
    Check,
    /// 安装 / 重启没成：不会自动重试，错误里写着该怎么办
    Install,
}

/// 最近一次成功更新：写进设置文件，重启后界面据此说「已更新」。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastUpdate {
    pub from: String,
    pub to: String,
    /// 毫秒时间戳
    pub at: u64,
}

/// 前端看到的全部：版本、能不能更新、开关、阶段、上次检查、失败原因。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UpdateStatus {
    /// 每次变化 +1：前端用它丢掉晚到的旧状态（先取状态、后收事件会乱序）
    pub seq: u64,
    pub current_version: String,
    /// `Some(原因)` = 这个进程不能自动更新（开发构建、不在 .app 里、被 macOS 临时挪了位置……）
    pub unavailable: Option<String>,
    pub auto_update: bool,
    pub phase: Phase,
    /// 最近一次检查的时间（毫秒时间戳），不论成败
    pub checked_at: Option<u64>,
    /// 新版本的更新说明（清单里的 `notes`），只在下载中 / 已就绪时有
    pub notes: Option<String>,
    /// 最近一次检查 / 下载 / 安装失败的原因；成功一次就清掉
    pub last_error: Option<String>,
    /// 上面那个失败属于哪一类（有 `last_error` 就有它）
    pub error_kind: Option<ErrorKind>,
    pub last_update: Option<LastUpdate>,
}

/// 正在做的事。
#[derive(Clone, Debug, PartialEq)]
enum Busy {
    Checking,
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Installing {
        version: String,
    },
}

/// 一次检查的结果。
#[derive(Debug, PartialEq)]
enum Outcome {
    UpToDate,
    /// 这个版本的安装包已经在内存里、验过签；`fresh` = 这次刚下载的（false = 早就在了，没重新下）
    Ready { version: String, fresh: bool },
}

/// 一次检查没成。`rejected` = 包下载下来了但验不过（签名 / 版本绑定不对）：
/// 这种失败往往是发布出了问题，30 分钟重试一次只会白白重下 5 MB，所以退避到下一个正常间隔。
#[derive(Debug, PartialEq)]
struct CheckFailure {
    message: String,
    rejected: bool,
}

/// 状态本体。只放字符串、数字，不放 `Update`，这样转换逻辑能脱离 Tauri 单测；
/// 安装包本体放在 `UpdateState::package`，和这里的 `ready` 同进同出。
struct Inner {
    seq: u64,
    busy: Option<Busy>,
    ready: Option<String>,
    up_to_date: bool,
    auto_update: bool,
    checked_at: Option<u64>,
    /// 距上一次尝试多久之后再检查（毫秒）：成功、签名类失败 = 正常间隔，网络类失败 = 30 分钟
    retry_wait_ms: u64,
    /// 正在下载的那个版本的更新说明；只在下载中显示，下载失败就丢
    download_notes: Option<String>,
    /// 已就绪（下载好、验过签）的那个版本的更新说明；跟着 `ready` 走，不会被别的版本的下载顶掉
    ready_notes: Option<String>,
    last_error: Option<String>,
    error_kind: Option<ErrorKind>,
    last_update: Option<LastUpdate>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            seq: 0,
            busy: None,
            ready: None,
            up_to_date: false,
            auto_update: true,
            checked_at: None,
            retry_wait_ms: CHECK_INTERVAL_MS,
            download_notes: None,
            ready_notes: None,
            last_error: None,
            error_kind: None,
            last_update: None,
        }
    }
}

impl Inner {
    fn phase(&self) -> Phase {
        // 真在下载 / 安装的优先；其次是「已下载好」；检查只是一瞬间的事，排在后面
        match &self.busy {
            Some(Busy::Downloading {
                version,
                downloaded,
                total,
            }) => {
                return Phase::Downloading {
                    version: version.clone(),
                    downloaded: *downloaded,
                    total: *total,
                };
            }
            Some(Busy::Installing { version }) => {
                return Phase::Installing {
                    version: version.clone(),
                };
            }
            _ => {}
        }
        if let Some(version) = &self.ready {
            return Phase::Ready {
                version: version.clone(),
            };
        }
        if matches!(self.busy, Some(Busy::Checking)) {
            return Phase::Checking;
        }
        if self.up_to_date {
            Phase::UpToDate
        } else {
            Phase::Idle
        }
    }

    /// 界面上显示哪个版本的更新说明：下载中显示正在下的那个，其余只显示「已就绪」的那个。
    fn notes(&self) -> Option<String> {
        if matches!(self.busy, Some(Busy::Downloading { .. })) {
            return self.download_notes.clone();
        }
        if self.ready.is_some() {
            return self.ready_notes.clone();
        }
        None
    }

    fn status(&self, current_version: String, unavailable: Option<String>) -> UpdateStatus {
        UpdateStatus {
            seq: self.seq,
            current_version,
            unavailable,
            auto_update: self.auto_update,
            phase: self.phase(),
            checked_at: self.checked_at,
            notes: self.notes(),
            last_error: self.last_error.clone(),
            error_kind: self.error_kind,
            last_update: self.last_update.clone(),
        }
    }

    /// 改状态，序号 +1。所有状态变化都走它：前端靠序号丢掉晚到的旧状态，漏了 +1 就会把新状态当旧的丢掉。
    fn mutate(&mut self, change: impl FnOnce(&mut Inner)) {
        change(self);
        self.seq += 1;
    }

    fn start_check(&mut self) {
        self.busy = Some(Busy::Checking);
    }

    fn start_download(&mut self, version: &str, notes: Option<String>) {
        self.download_notes = notes;
        self.busy = Some(Busy::Downloading {
            version: version.to_string(),
            downloaded: 0,
            total: None,
        });
    }

    fn progress(&mut self, downloaded: u64, total: Option<u64>) {
        if let Some(Busy::Downloading {
            downloaded: old_downloaded,
            total: old_total,
            ..
        }) = &mut self.busy
        {
            *old_downloaded = downloaded;
            *old_total = total;
        }
    }

    /// 一次检查（含下载）做完了，不论成败。
    fn finish_check(&mut self, now: u64, result: Result<Outcome, CheckFailure>) {
        self.busy = None;
        self.checked_at = Some(now);
        let downloaded_notes = self.download_notes.take();
        match result {
            Ok(Outcome::UpToDate) => {
                self.ready = None;
                self.ready_notes = None;
                self.up_to_date = true;
                self.retry_wait_ms = CHECK_INTERVAL_MS;
                self.last_error = None;
                self.error_kind = None;
            }
            Ok(Outcome::Ready { version, fresh }) => {
                // 刚下载的：说明跟着它走；早就在的：它自己的说明原样留着
                if fresh {
                    self.ready_notes = downloaded_notes;
                }
                self.ready = Some(version);
                self.up_to_date = false;
                self.retry_wait_ms = CHECK_INTERVAL_MS;
                self.last_error = None;
                self.error_kind = None;
            }
            Err(failure) => {
                // 已经下载好的版本不受影响（`ready` 与它的说明都不动）：这次失败只是没能查到更新的更新
                self.up_to_date = false;
                self.retry_wait_ms = if failure.rejected {
                    CHECK_INTERVAL_MS
                } else {
                    RETRY_AFTER_FAILURE_MS
                };
                self.last_error = Some(failure.message);
                self.error_kind = Some(ErrorKind::Check);
            }
        }
    }

    fn start_install(&mut self, version: &str) {
        self.busy = Some(Busy::Installing {
            version: version.to_string(),
        });
    }

    /// 安装没成。`keep_ready` = 安装包还能再试一次（比如用户取消了管理员授权）。
    fn install_failed(&mut self, message: String, keep_ready: bool) {
        self.busy = None;
        self.last_error = Some(message);
        self.error_kind = Some(ErrorKind::Install);
        if !keep_ready {
            self.ready = None;
            self.ready_notes = None;
        }
    }
}

/// 下载好、验过签的安装包。
struct Package {
    update: Update,
    bytes: Vec<u8>,
}

pub struct UpdateState {
    inner: Mutex<Inner>,
    package: Mutex<Option<Package>>,
    /// 同一时刻只做一件事（检查 / 下载 / 安装）：后台检查、手动检查、安装互不撞车。
    /// `try_lock` 拿不到 = 已经有一件在做，调用方直接返回。
    gate: tokio::sync::Mutex<()>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            package: Mutex::new(None),
            gate: tokio::sync::Mutex::new(()),
        }
    }
}

impl UpdateState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn package(&self) -> std::sync::MutexGuard<'_, Option<Package>> {
        self.package
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn status(&self, app: &AppHandle) -> UpdateStatus {
        self.lock().status(app_version(app), unavailable_now())
    }

    /// 改状态并通知前端。锁在发事件之前放掉：事件处理里可能回头来取状态。
    fn update(&self, app: &AppHandle, change: impl FnOnce(&mut Inner)) -> UpdateStatus {
        let snapshot = {
            let mut inner = self.lock();
            inner.mutate(change);
            inner.status(app_version(app), unavailable_now())
        };
        let _ = app.emit(EVENT, &snapshot);
        snapshot
    }
}

fn app_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 该不该检查了。用挂钟时间，不用 tokio 的计时器：Mac 睡眠时单调时钟不走，
/// 合盖睡一夜，6 小时的计时器醒来还要再等 6 小时。
fn due(now: u64, last_attempt: Option<u64>, wait_ms: u64) -> bool {
    let Some(last) = last_attempt else {
        return true;
    };
    // 系统时间被往回调过：别傻等到「未来」
    if now < last {
        return true;
    }
    now - last >= wait_ms
}

/// 这个进程能不能被自动更新。返回 `Some(原因)` = 不能。
///
/// 插件把「可执行文件所在的 .app」整个换掉；不是从 .app 里跑的（`cargo run`、开发构建）它会换掉可执行文件
/// 所在的整个目录，开发目录就毁了。从下载目录 / 磁盘映像直接打开的，macOS 会把它挪到只读的临时位置
/// （App Translocation），也装不上——最常见的就是新电脑上第一次安装。
fn unavailable_reason(exe: &Path, debug_build: bool) -> Option<String> {
    if debug_build {
        return Some("开发版（debug 构建）不检查更新".to_string());
    }
    let path = exe.to_string_lossy();
    if path.contains("/AppTranslocation/") || path.starts_with("/Volumes/") {
        return Some(
            "SENSE 现在是从下载目录、磁盘映像或外置磁盘里直接打开的，更新装不进去（系统把它放在只读位置，或者它不在这台电脑的系统盘上）。\
             请先把它拖进「应用程序」文件夹，再从那里打开"
                .to_string(),
        );
    }
    if !path.contains(".app/Contents/MacOS/") {
        return Some("不是从 .app 里运行的，不自动更新".to_string());
    }
    None
}

/// 开发期开关，只在 debug 构建里生效（同 `window::dev_flag`），发布版里它们不存在：
/// - `SENSE_DEV_UPDATE=1`：debug 构建也检查更新。前提仍是二进制装在 `.app` 里（见 `unavailable_reason`），
///   裸的 `target/debug/sense` 照样拒绝——插件会换掉可执行文件所在的整个目录
/// - `SENSE_DEV_UPDATE_RESTART=1`：下载好、验过签之后不等用户点按钮，直接走「立即重启」同一个函数。
///   给无人值守的端到端验证用（用 debug 构建打成 .app，对着本机的假发布源）
const DEV_UPDATE: &str = "SENSE_DEV_UPDATE";
const DEV_UPDATE_RESTART: &str = "SENSE_DEV_UPDATE_RESTART";

fn unavailable_now() -> Option<String> {
    match std::env::current_exe() {
        Ok(exe) => unavailable_reason(&exe, cfg!(debug_assertions) && !window::dev_flag(DEV_UPDATE)),
        Err(err) => Some(format!("取不到自己所在的位置：{err}")),
    }
}

/// 把插件的错误翻成用户看得懂的话。每条都说清「发生了什么」和「有没有装进去任何东西」。
fn friendly(err: &tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error as E;
    match err {
        E::ReleaseNotFound => {
            "取不到更新清单（GitHub 上还没有发布版本，或者网络不通）".to_string()
        }
        E::Reqwest(inner) if inner.is_timeout() => {
            // 更新检查和模型请求一样不读 macOS 系统代理（见 Cargo.toml 里更新插件那一行的注释）：别指望用户去「配代理」
            "连接 GitHub 超时（网络慢；注意：更新检查不读系统代理）".to_string()
        }
        E::Reqwest(inner) if inner.is_connect() => {
            "连不上 GitHub（网络不通；注意：更新检查不读系统代理）".to_string()
        }
        E::Reqwest(inner) if inner.is_decode() => {
            "更新清单的内容不对（不是合法的 JSON，可能拿到了一个错误页）".to_string()
        }
        E::Reqwest(inner) => format!("网络出错：{inner}"),
        E::Network(message) => format!("下载失败：{message}"),
        E::TargetNotFound(target) => {
            format!("最新发布里没有适配这台电脑的安装包（{target}）")
        }
        E::TargetsNotFound(targets) => {
            format!("最新发布里没有适配这台电脑的安装包（{}）", targets.join("、"))
        }
        E::Minisign(_) | E::Base64(_) | E::SignatureUtf8(_) => {
            "更新包的签名校验没通过，已丢弃，没有装任何东西".to_string()
        }
        E::SignedVersionMismatch { signed, announced } => format!(
            "更新包签名的版本（{signed}）和清单写的版本（{announced}）对不上，已丢弃，没有装任何东西"
        ),
        E::MissingSignedVersion => {
            "更新包的签名没有绑定版本号，已丢弃，没有装任何东西".to_string()
        }
        E::PreviousAppNotRestored(path) => format!(
            "安装中途失败，而且旧版本没能放回原位。旧版本在 {}，请手动把它拖回「应用程序」文件夹",
            path.display()
        ),
        // macOS 上走特权安装（弹管理员授权）时，用户取消、授权没通过、脚本出错，插件一律返回
        // `Io(PermissionDenied)`；`AuthenticationFailed` 只在 Linux 上出现。所以这一条要把「取消了授权」也说在里面。
        E::AuthenticationFailed => "替换应用需要管理员授权，授权被取消了".to_string(),
        E::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied => {
            "没能替换「应用程序」里的 SENSE：可能是你取消了管理员授权，也可能是当前账户没有权限（请用管理员账户登录，或手动安装新版本）".to_string()
        }
        other => other.to_string(),
    }
}

/// 包下载下来了，但验不过（签名 / 版本绑定不对）。这类失败多半是发布出了问题，重试也不会好，
/// 所以下次检查退避到正常间隔，别每 30 分钟白白重下一遍。
fn is_rejection(err: &tauri_plugin_updater::Error) -> bool {
    use tauri_plugin_updater::Error as E;
    matches!(
        err,
        E::Minisign(_)
            | E::Base64(_)
            | E::SignatureUtf8(_)
            | E::SignedVersionMismatch { .. }
            | E::MissingSignedVersion
    )
}

fn failure_from(err: &tauri_plugin_updater::Error) -> CheckFailure {
    CheckFailure {
        message: friendly(err),
        rejected: is_rejection(err),
    }
}

/// 安装失败之后，内存里的安装包还能不能再试一次。只有「旧版本没能放回原位」不能：那时已经乱了，
/// 再试可能把事情搞得更糟，只能让用户按提示手动处理。其余（取消授权、没有权限、磁盘问题）都可以重试。
fn install_error_is_retryable(err: &tauri_plugin_updater::Error) -> bool {
    !matches!(err, tauri_plugin_updater::Error::PreviousAppNotRestored(_))
}

/// 点「立即重启」时闸门被别的任务占着：按它在做什么决定怎么回话。`None` = 是检查（几秒钟的事），等它做完再装。
fn busy_reply(busy: Option<&Busy>) -> Option<String> {
    match busy {
        Some(Busy::Downloading { version, .. }) => Some(format!("正在下载更新 v{version}，下载完再点")),
        Some(Busy::Installing { .. }) => Some("正在安装，不用重复点".to_string()),
        _ => None,
    }
}

/// 清单里写的下载地址必须和清单本身同源（同协议、同主机、同端口）：清单没有签名，被人改成指向别处，
/// 我们就会去拉任意地址的一大坨东西（装不上，但白耗流量和内存）。正式端点是 github.com，下载地址也在 github.com 下。
fn same_origin(left: &str, right: &str) -> bool {
    match (reqwest::Url::parse(left), reqwest::Url::parse(right)) {
        (Ok(left), Ok(right)) => {
            left.scheme() == right.scheme()
                && left.host_str() == right.host_str()
                && left.port_or_known_default() == right.port_or_known_default()
        }
        _ => false,
    }
}

/// 下载地址和清单不同源就拒收（见 `same_origin`）；读不到配置里的端点就不校验（插件自己没有端点也建不起来）。
fn check_download_origin(download_url: &str, endpoint: Option<&str>) -> Result<(), CheckFailure> {
    match endpoint {
        Some(endpoint) if !same_origin(download_url, endpoint) => Err(CheckFailure {
            message: "更新清单里的下载地址和清单不是同一个来源，已拒绝，没有下载任何东西".to_string(),
            rejected: true,
        }),
        _ => Ok(()),
    }
}

/// 检查说「已下载好」，内存里却没有安装包——下载之后存包那一步出了毛病。等到点「立即重启」才报
/// 「没有已经下载好的更新」就太晚了，用户会以为是自己哪里没点对：在这里就当检查失败，说成内部错误。
fn reconcile(result: Result<Outcome, CheckFailure>, has_package: bool) -> Result<Outcome, CheckFailure> {
    match result {
        Ok(Outcome::Ready { .. }) if !has_package => Err(CheckFailure {
            message: "内部错误：更新包下载好了却没有留住，请再点一次「检查更新」；还不行就手动下载新版本".to_string(),
            rejected: false,
        }),
        other => other,
    }
}

/// 配置里的更新清单地址（`plugins.updater.endpoints[0]`）。
fn configured_endpoint(app: &AppHandle) -> Option<String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("endpoints"))
        .and_then(|endpoints| endpoints.get(0))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 设置文件里的自动更新开关：没写 / 写的不是布尔值（比如手改成了 "false" 字符串）= 开。
fn parse_auto_update(settings: Option<&Value>) -> bool {
    settings
        .and_then(|doc| doc.get(AUTO_UPDATE_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn truncate_chars(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let mut cut: String = trimmed.chars().take(max).collect();
    cut.push('…');
    cut
}

/// 启动：读开关，开着就起后台检查。不能更新的环境（开发版等）只打一行日志，不起任务。
pub fn start(app: &AppHandle) {
    let state = app.state::<UpdateState>();
    let doc = settings::read_value(app);
    if let Err(err) = &doc {
        // 设置文件读不出来（损坏、没权限）：开关按「开」处理，但要说出来，别让人以为是自己设的
        eprintln!("[sense] update: 读不出设置，自动检查按默认的「开」：{err}");
    }
    let auto_update = parse_auto_update(doc.as_ref().ok());
    let last_update = doc
        .as_ref()
        .ok()
        .and_then(|value| value.get(LAST_UPDATE_KEY).cloned())
        .and_then(|value| serde_json::from_value::<LastUpdate>(value).ok());
    state.update(app, |inner| {
        inner.auto_update = auto_update;
        inner.last_update = last_update;
    });

    if let Some(reason) = unavailable_now() {
        println!("[sense] update: 不检查更新——{reason}");
        return;
    }
    println!(
        "[sense] update: 当前 v{}，自动检查{}（启动 {} 秒后第一次，之后每 6 小时）",
        app_version(app),
        if auto_update { "开着" } else { "关着" },
        FIRST_CHECK_DELAY.as_secs()
    );

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            let (enabled, last_attempt, wait_ms) = {
                let state = app.state::<UpdateState>();
                let inner = state.lock();
                (inner.auto_update, inner.checked_at, inner.retry_wait_ms)
            };
            if enabled && due(now_ms(), last_attempt, wait_ms) {
                run_check(&app, false).await;
                if window::dev_flag(DEV_UPDATE_RESTART) {
                    let state = app.state::<UpdateState>();
                    let ready = state.lock().ready.is_some();
                    if ready {
                        println!("[sense] update: {DEV_UPDATE_RESTART} 已设置，直接安装并重启");
                        if let Err(message) = install_and_relaunch(&app).await {
                            eprintln!("[sense] update: 安装并重启失败：{message}");
                        }
                    }
                }
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

/// 一次完整的「检查 → 下载 → 验签」。已经在做别的事就直接返回。
/// `manual` = 用户点了「检查更新」，只影响日志；失败的原因都进状态，界面上看得到。
pub async fn run_check(app: &AppHandle, manual: bool) {
    let state = app.state::<UpdateState>();
    let Ok(_gate) = state.gate.try_lock() else {
        return;
    };
    if let Some(reason) = unavailable_now() {
        eprintln!("[sense] update: 不检查更新——{reason}");
        return;
    }
    let source = if manual { "手动" } else { "后台" };
    state.update(app, |inner| inner.start_check());

    let result = check_and_download(app, &state).await;
    let result = reconcile(result, state.package().is_some());
    match &result {
        Ok(Outcome::UpToDate) => {
            println!("[sense] update: {source}检查——已是最新（v{}）", app_version(app));
        }
        Ok(Outcome::Ready { version, .. }) => {
            println!("[sense] update: {source}检查——v{version} 已下载并验过签，等用户重启");
        }
        Err(failure) => eprintln!("[sense] update: {source}检查失败：{}", failure.message),
    }
    state.update(app, |inner| inner.finish_check(now_ms(), result));
}

async fn check_and_download(
    app: &AppHandle,
    state: &UpdateState,
) -> Result<Outcome, CheckFailure> {
    let updater = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|err| failure_from(&err))?;
    let found = updater.check().await.map_err(|err| failure_from(&err))?;

    let Some(mut update) = found else {
        // 清单里没有比现在更新的版本。之前下载好的那个多半被撤回了（发布者把它改成了预发布 / 删了）：
        // 不能再装，丢掉。
        if state.package().take().is_some() {
            println!("[sense] update: 之前下载好的版本已经不在清单里（被撤回？），丢弃");
        }
        return Ok(Outcome::UpToDate);
    };

    // 下载地址必须和清单同源：清单没有签名，被改成指向别处就不去拉
    check_download_origin(update.download_url.as_str(), configured_endpoint(app).as_deref())?;

    let version = update.version.clone();
    if state.lock().ready.as_deref() == Some(version.as_str()) && state.package().is_some() {
        return Ok(Outcome::Ready {
            version,
            fresh: false,
        });
    }

    println!(
        "[sense] update: 发现 v{version}（当前 v{}），开始下载",
        update.current_version
    );
    let notes = update
        .body
        .as_deref()
        .map(|body| truncate_chars(body, NOTES_MAX_CHARS))
        .filter(|body| !body.is_empty());
    state.update(app, |inner| inner.start_download(&version, notes));

    // 下载很久，网络差时可能几分钟；取清单那 20 秒的时限不适用
    update.timeout = Some(DOWNLOAD_TIMEOUT);
    let mut downloaded: u64 = 0;
    let mut last_push = Instant::now()
        .checked_sub(PROGRESS_EVERY)
        .unwrap_or_else(Instant::now);
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                if last_push.elapsed() >= PROGRESS_EVERY {
                    last_push = Instant::now();
                    state.update(app, |inner| inner.progress(downloaded, total));
                }
            },
            || {},
        )
        .await
        .map_err(|err| failure_from(&err))?;

    // `download` 已经用内置公钥验过签（含版本绑定）；走到这里的字节才是可信的
    *state.package() = Some(Package { update, bytes });
    Ok(Outcome::Ready {
        version,
        fresh: true,
    })
}

/// 装上已下载的更新并重启。
async fn install_and_relaunch(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<UpdateState>();
    let _gate = match state.gate.try_lock() {
        Ok(gate) => gate,
        Err(_) => {
            // 在做什么决定怎么办：下载可能很久，不等；安装已经在进行，别重复点；
            // 检查只是几秒钟（后台复查时前端看不到它，横幅还亮着），等它做完再装
            let busy = state.lock().busy.clone();
            if let Some(reply) = busy_reply(busy.as_ref()) {
                return Err(reply);
            }
            match tokio::time::timeout(CHECK_TIMEOUT + Duration::from_secs(5), state.gate.lock())
                .await
            {
                Ok(gate) => gate,
                Err(_) => return Err("正在检查更新，稍等一下再点".to_string()),
            }
        }
    };
    if let Some(reason) = unavailable_now() {
        return Err(reason);
    }
    let Some(package) = state.package().take() else {
        return Err("没有已经下载好的更新".to_string());
    };
    let version = package.update.version.clone();
    let from = package.update.current_version.clone();
    println!("[sense] update: 安装 v{version}（当前 v{from}）");
    state.update(app, |inner| inner.start_install(&version));

    // `install` 是同步的，macOS 上没有写权限时还要请主线程弹管理员授权并等它——
    // 在主线程（同步命令就跑在主线程）上调会自己等自己。所以放到阻塞线程池里。
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let result = package.update.install(&package.bytes);
        (result, package)
    })
    .await;
    let (result, package) = match joined {
        Ok(pair) => pair,
        Err(err) => {
            let message = format!("安装线程异常退出：{err}");
            eprintln!("[sense] update: {message}");
            state.update(app, |inner| inner.install_failed(message.clone(), false));
            return Err(message);
        }
    };

    if let Err(err) = result {
        let message = friendly(&err);
        eprintln!("[sense] update: 安装失败：{message}");
        // 旧版本没能放回原位时，内存里的安装包也没法再试；其余情况（取消授权、没权限）留着让用户重试
        let retryable = install_error_is_retryable(&err);
        if retryable {
            *state.package() = Some(package);
        }
        state.update(app, |inner| inner.install_failed(message.clone(), retryable));
        return Err(message);
    }

    record_last_update(app, &from, &version);
    println!("[sense] update: v{version} 已装好，重启");
    if let Err(message) = relaunch(app) {
        // 新版本已经在磁盘上了，只是没能自动拉起来：照实说，让用户自己打开
        let message = format!("新版本已经装好，但没能自动重启（{message}）。请退出 SENSE 后重新打开");
        eprintln!("[sense] update: {message}");
        state.update(app, |inner| inner.install_failed(message.clone(), false));
        return Err(message);
    }
    Ok(())
}

fn record_last_update(app: &AppHandle, from: &str, to: &str) {
    let record = LastUpdate {
        from: from.to_string(),
        to: to.to_string(),
        at: now_ms(),
    };
    let value = serde_json::to_value(&record).unwrap_or(Value::Null);
    if let Err(err) = settings::update(app, |map| {
        map.insert(LAST_UPDATE_KEY.to_string(), value);
    }) {
        // 只是少一行「最近更新」的记录，不影响更新本身
        eprintln!("[sense] update: 记录最近更新失败：{err}");
    }
    app.state::<UpdateState>()
        .update(app, |inner| inner.last_update = Some(record));
}

/// 等这个进程退出之后，再用系统的 `open -n` 把新装的 `.app` 拉起来，并确认它真的起来了。
///
/// 参数（都当单个 argv 传，从不拼进脚本文本，路径里有空格、引号、`$`、反斜杠都不怕）：
/// `$1` 旧进程的 pid；`$2` 新装的 `.app` 路径；`$3` 用来打开它的程序（正式是 `/usr/bin/open`，测试里换成假的）；
/// `$4` 每次尝试最多等多少个 0.2 秒；`$5` 等旧进程退出最多等多少个 0.1 秒。
///
/// - 不用 `AppHandle::restart`（它先拉起子进程再退出）：两个进程会同时存在一小会儿，新进程注册全局快捷键时
///   旧进程还占着，注册会失败。`open` 走 LaunchServices，新窗口也会被正确激活、拿到键盘焦点。
/// - **一定要 `-n`**：旧进程刚退出时 LaunchServices 往往还当它在运行；这时普通的 `open` 去「激活已有实例」
///   （系统日志里是 `Asking CSUI to launch 0 items`），根本不会启动新的——装在 /Applications 里实测撞上过：
///   换 `.app` 成功、旧进程退出了，新进程却没有起来。`-n` 无论如何都启动一个新实例。
/// - **旧进程不退就不开新的**：等不到它退出（卡死了），再开一个会和它抢全局快捷键、还可能把卡死的那个误当成「新进程」。
///   这时只记一行日志。
/// - 起没起来，看 `ps` 里有没有命令行正好是 `<app>/Contents/MacOS/sense`（后面可以跟参数）的进程：
///   必须从头比、整条比，`ps | grep` 会匹配到自己，`sense-ocr` 之类的子进程也不能算数。路径通过环境变量交给 awk，
///   不经过 `-v`（`-v` 会处理反斜杠转义，路径里有 `\` 就永远匹配不上，然后连开三个实例）。
///   没起来再 `open -n` 一次，最多 3 次；三次都没起来，写一行到 `~/Library/Logs/SENSE-update.log`。
const RELAUNCH_SCRIPT: &str = r#"
log() {
  mkdir -p "$HOME/Library/Logs" 2>/dev/null
  echo "$(date '+%Y-%m-%d %H:%M:%S') $1" >> "$HOME/Library/Logs/SENSE-update.log"
}
i=0
while kill -0 "$1" 2>/dev/null; do
  i=$((i+1))
  if [ "$i" -ge "${5:-300}" ]; then
    log "更新已装好，但旧进程 $1 一直没有退出，没有自动重启。请退出 SENSE 后重新打开"
    exit 1
  fi
  sleep 0.1
done
n=0
while [ "$n" -lt 3 ]; do
  "$3" -n "$2"
  j=0
  while [ "$j" -lt "${4:-100}" ]; do
    /bin/ps -ax -o command= | P="$2/Contents/MacOS/sense" /usr/bin/awk 'BEGIN { p = ENVIRON["P"] } $0 == p || index($0, p " ") == 1 { found = 1 } END { exit !found }' && exit 0
    j=$((j+1))
    sleep 0.2
  done
  n=$((n+1))
done
log "更新已装好，但 open -n 试了 3 次，$2 里的 sense 都没有起来。请手动打开 SENSE"
exit 1
"#;

/// 每次 `open -n` 之后最多等多久（0.2 秒的个数）：首次启动偏慢的机器上 20 秒够了。
const RELAUNCH_POLLS: u32 = 100;
/// 等旧进程退出最多等多久（0.1 秒的个数）：30 秒。
const RELAUNCH_WAIT_OLD: u32 = 300;

fn relaunch(app: &AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let bundle = tauri_plugin_updater::extract_path_from_executable(&exe)
        .map_err(|err| err.to_string())?;
    Command::new("/bin/sh")
        .arg("-c")
        .arg(RELAUNCH_SCRIPT)
        .arg("sense-relaunch")
        .arg(std::process::id().to_string())
        .arg(&bundle)
        .arg("/usr/bin/open")
        .arg(RELAUNCH_POLLS.to_string())
        .arg(RELAUNCH_WAIT_OLD.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // 自成一个进程组：我们退出时不连带它
        .process_group(0)
        .spawn()
        .map_err(|err| err.to_string())?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub fn update_status(window: WebviewWindow, app: AppHandle) -> Result<UpdateStatus, String> {
    window::ensure_main(&window)?;
    Ok(app.state::<UpdateState>().status(&app))
}

/// 立刻检查一次（发现新版本就一直下载到验签完成）。进度走事件；这个调用等到全部做完才返回。
/// 已经在检查 / 下载时直接返回当前状态。
#[tauri::command]
pub async fn update_check_now(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<UpdateStatus, String> {
    window::ensure_main(&window)?;
    run_check(&app, true).await;
    Ok(app.state::<UpdateState>().status(&app))
}

#[tauri::command]
pub async fn update_restart(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    window::ensure_main(&window)?;
    install_and_relaunch(&app).await
}

#[tauri::command]
pub fn set_auto_update(
    window: WebviewWindow,
    app: AppHandle,
    enabled: bool,
) -> Result<UpdateStatus, String> {
    window::ensure_main(&window)?;
    // 先落盘再改内存：写不进去就如实报错，别让界面显示一个重启后会变回去的开关
    settings::update(&app, |map| {
        map.insert(AUTO_UPDATE_KEY.to_string(), json!(enabled));
    })?;
    let state = app.state::<UpdateState>();
    Ok(state.update(&app, |inner| inner.auto_update = enabled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const MIN: u64 = 60 * 1000;
    const HOUR: u64 = 60 * MIN;

    // ---------- 到点没有 ----------

    #[test]
    fn first_check_is_always_due() {
        assert!(due(1_000, None, CHECK_INTERVAL_MS));
        assert!(due(1_000, None, RETRY_AFTER_FAILURE_MS));
    }

    #[test]
    fn next_check_waits_as_long_as_the_state_says() {
        let last = 10 * HOUR;
        assert!(!due(last + 6 * HOUR - 1, Some(last), CHECK_INTERVAL_MS));
        assert!(due(last + 6 * HOUR, Some(last), CHECK_INTERVAL_MS));
        assert!(!due(last + 30 * MIN - 1, Some(last), RETRY_AFTER_FAILURE_MS));
        assert!(due(last + 30 * MIN, Some(last), RETRY_AFTER_FAILURE_MS));
        // 失败后的重试间隔必须比正常间隔短，否则「失败重试」没有意义
        assert!(RETRY_AFTER_FAILURE_MS < CHECK_INTERVAL_MS);
    }

    #[test]
    fn clock_moved_backwards_does_not_stall_checks() {
        assert!(due(5 * HOUR, Some(9 * HOUR), CHECK_INTERVAL_MS));
    }

    // ---------- 能不能在这里更新 ----------

    #[test]
    fn installed_app_can_update() {
        let exe = PathBuf::from("/Applications/SENSE.app/Contents/MacOS/sense");
        assert_eq!(unavailable_reason(&exe, false), None);
        let user_apps = PathBuf::from("/Users/a b/Applications/SENSE.app/Contents/MacOS/sense");
        assert_eq!(unavailable_reason(&user_apps, false), None);
    }

    #[test]
    fn dev_builds_and_loose_binaries_never_update() {
        // 开发构建：插件会把可执行文件所在的整个目录换掉，必须挡住
        let loose = PathBuf::from("/work/sense/src-tauri/target/release/sense");
        assert!(unavailable_reason(&loose, false).unwrap().contains("不是从 .app 里运行"));
        let app = PathBuf::from("/Applications/SENSE.app/Contents/MacOS/sense");
        assert!(unavailable_reason(&app, true).unwrap().contains("开发版"));
    }

    #[test]
    fn translocated_or_mounted_app_is_explained_not_attempted() {
        let translocated = PathBuf::from(
            "/private/var/folders/ab/xyz/T/AppTranslocation/8E1C/d/SENSE.app/Contents/MacOS/sense",
        );
        let reason = unavailable_reason(&translocated, false).expect("应当拒绝");
        assert!(reason.contains("应用程序"), "要告诉用户怎么办: {reason}");
        let mounted = PathBuf::from("/Volumes/SENSE/SENSE.app/Contents/MacOS/sense");
        let reason = unavailable_reason(&mounted, false).expect("磁盘映像里不能更新");
        // 外置磁盘上的 app 也会被拒，提示不能只说「下载目录 / 磁盘映像」
        assert!(reason.contains("外置磁盘"), "{reason}");
    }

    // ---------- 错误翻译 ----------

    #[test]
    fn friendly_errors_say_what_happened_and_what_was_installed() {
        use tauri_plugin_updater::Error as E;
        assert!(friendly(&E::ReleaseNotFound).contains("更新清单"));
        let missing = friendly(&E::TargetNotFound("darwin-x86_64".into()));
        assert!(missing.contains("darwin-x86_64") && missing.contains("适配"), "{missing}");
        let mismatch = friendly(&E::SignedVersionMismatch {
            signed: "0.1.1".into(),
            announced: "0.1.9".into(),
        });
        assert!(mismatch.contains("0.1.1") && mismatch.contains("0.1.9") && mismatch.contains("没有装"), "{mismatch}");
        assert!(friendly(&E::MissingSignedVersion).contains("没有装"));
        let stuck = friendly(&E::PreviousAppNotRestored(PathBuf::from("/Applications/SENSE (previous version).app")));
        assert!(stuck.contains("previous version") && stuck.contains("拖回"), "{stuck}");
        assert!(friendly(&E::AuthenticationFailed).contains("授权"));
        assert!(friendly(&E::TargetsNotFound(vec!["darwin-aarch64-app".into(), "darwin-aarch64".into()])).contains("darwin-aarch64-app、darwin-aarch64"));
        // macOS 上特权安装的失败（含用户取消授权）一律是 PermissionDenied：文案要把「取消了授权」也说在里面
        let denied = friendly(&E::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
        assert!(denied.contains("取消了管理员授权") && denied.contains("没有权限"), "{denied}");
        // 没特别处理的错误原样给出，不吞
        assert_eq!(friendly(&E::EmptyEndpoints), "Updater does not have any endpoints set.");
    }

    /// 拿到的不是 JSON（GitHub 的错误页、被代理改写的页面）：要说「清单内容不对」，不能说成网络出错
    #[tokio::test]
    async fn a_manifest_that_is_not_json_is_not_reported_as_a_network_error() {
        use tauri::http::Response;
        let response = reqwest::Response::from(Response::new("<html>502 Bad Gateway</html>"));
        let err = response.json::<Value>().await.expect_err("不是 JSON，解码必须失败");
        let message = friendly(&tauri_plugin_updater::Error::Reqwest(err));
        assert!(message.contains("更新清单的内容不对"), "{message}");
        assert!(!message.contains("网络出错"), "{message}");
    }

    #[test]
    fn notes_are_trimmed_and_capped_on_character_boundaries() {
        assert_eq!(truncate_chars("  修了几个 bug \n", 600), "修了几个 bug");
        let long = "字".repeat(700);
        let cut = truncate_chars(&long, 600);
        assert_eq!(cut.chars().count(), 601);
        assert!(cut.ends_with('…'));
    }

    // ---------- 状态转换 ----------

    fn status_of(inner: &Inner) -> UpdateStatus {
        inner.status("0.1.0".into(), None)
    }

    fn ready(version: &str, fresh: bool) -> Outcome {
        Outcome::Ready {
            version: version.to_string(),
            fresh,
        }
    }

    fn failure(message: &str, rejected: bool) -> CheckFailure {
        CheckFailure {
            message: message.to_string(),
            rejected,
        }
    }

    #[test]
    fn fresh_state_is_idle_with_auto_update_on() {
        let status = status_of(&Inner::default());
        assert_eq!(status.phase, Phase::Idle);
        assert!(status.auto_update);
        assert_eq!(status.checked_at, None);
    }

    #[test]
    fn download_progress_then_ready_then_recheck_keeps_the_banner() {
        let mut inner = Inner::default();
        inner.start_check();
        assert_eq!(inner.phase(), Phase::Checking);

        inner.start_download("0.1.1", Some("修了 bug".into()));
        inner.progress(1024, Some(4096));
        assert_eq!(
            inner.phase(),
            Phase::Downloading { version: "0.1.1".into(), downloaded: 1024, total: Some(4096) }
        );

        inner.finish_check(100, Ok(ready("0.1.1", true)));
        assert_eq!(inner.phase(), Phase::Ready { version: "0.1.1".into() });
        assert_eq!(status_of(&inner).notes.as_deref(), Some("修了 bug"));

        // 6 小时后的后台复查：检查进行中、以及复查失败，横幅都不能掉
        inner.start_check();
        assert_eq!(inner.phase(), Phase::Ready { version: "0.1.1".into() });
        inner.finish_check(200, Err(failure("连不上 GitHub", false)));
        assert_eq!(inner.phase(), Phase::Ready { version: "0.1.1".into() });
        assert_eq!(inner.last_error.as_deref(), Some("连不上 GitHub"));
        assert_eq!(inner.retry_wait_ms, RETRY_AFTER_FAILURE_MS);
    }

    #[test]
    fn a_withdrawn_release_clears_the_ready_state() {
        let mut inner = Inner::default();
        inner.start_download("0.1.1", Some("说明".into()));
        inner.finish_check(1, Ok(ready("0.1.1", true)));
        inner.finish_check(2, Ok(Outcome::UpToDate));
        assert_eq!(inner.phase(), Phase::UpToDate);
        assert_eq!(inner.ready, None);
        assert_eq!(status_of(&inner).notes, None);
        assert_eq!(inner.ready_notes, None, "撤回的版本，说明也不能留着");
    }

    #[test]
    fn failure_is_reported_not_swallowed_and_success_clears_it() {
        let mut inner = Inner::default();
        inner.start_check();
        inner.finish_check(5, Err(failure("取不到更新清单", false)));
        let status = status_of(&inner);
        assert_eq!(status.phase, Phase::Idle, "失败不能冒充「已是最新」");
        assert_eq!(status.last_error.as_deref(), Some("取不到更新清单"));
        assert_eq!(status.error_kind, Some(ErrorKind::Check));
        assert_eq!(status.checked_at, Some(5));

        inner.finish_check(6, Ok(Outcome::UpToDate));
        let status = status_of(&inner);
        assert_eq!(status.phase, Phase::UpToDate);
        assert_eq!(status.last_error, None);
        assert_eq!(status.error_kind, None, "成功一次，失败的原因和类别一起清掉");
        assert_eq!(inner.retry_wait_ms, CHECK_INTERVAL_MS);
    }

    #[test]
    fn progress_outside_a_download_is_ignored() {
        let mut inner = Inner::default();
        inner.progress(10, Some(20));
        assert_eq!(inner.phase(), Phase::Idle);
        inner.start_check();
        inner.progress(10, Some(20));
        assert_eq!(inner.phase(), Phase::Checking);
    }

    #[test]
    fn failed_install_keeps_or_drops_the_package_as_told() {
        let mut inner = Inner::default();
        inner.start_download("0.1.1", Some("说明".into()));
        inner.finish_check(1, Ok(ready("0.1.1", true)));
        inner.start_install("0.1.1");
        assert_eq!(inner.phase(), Phase::Installing { version: "0.1.1".into() });

        // 用户取消了管理员授权：安装包还在，回到「已就绪」，可以再点；这是安装类失败，不会自动重试
        inner.install_failed("授权被取消".into(), true);
        assert_eq!(inner.phase(), Phase::Ready { version: "0.1.1".into() });
        assert_eq!(inner.last_error.as_deref(), Some("授权被取消"));
        assert_eq!(inner.error_kind, Some(ErrorKind::Install));
        assert_eq!(status_of(&inner).notes.as_deref(), Some("说明"), "重试期间说明还在");

        // 旧版本没放回去：这个安装包不能再试，说明也一起丢
        inner.start_install("0.1.1");
        inner.install_failed("旧版本留在别处".into(), false);
        assert_eq!(inner.phase(), Phase::Idle);
        assert_eq!(inner.ready_notes, None);
        assert_eq!(inner.error_kind, Some(ErrorKind::Install));
    }

    // ---------- 与前端的契约 ----------

    /// 前端的 zod 解析（src/lib/updateStatus.test.ts）读的是同一份夹具：
    /// 这边改了字段而夹具没改，这个测试红；夹具改了而前端解析不了，那边的测试红。
    #[test]
    fn status_json_matches_the_frontend_fixture() {
        let base = |phase: Phase| UpdateStatus {
            seq: 7,
            current_version: "0.1.0".into(),
            unavailable: None,
            auto_update: true,
            phase,
            checked_at: Some(1_767_225_600_000),
            notes: None,
            last_error: None,
            error_kind: None,
            last_update: None,
        };
        let cases: Vec<(&str, UpdateStatus)> = vec![
            ("idle", UpdateStatus { checked_at: None, ..base(Phase::Idle) }),
            ("checking", base(Phase::Checking)),
            ("up_to_date", base(Phase::UpToDate)),
            (
                "downloading",
                UpdateStatus {
                    notes: Some("修了几个 bug".into()),
                    ..base(Phase::Downloading { version: "0.1.1".into(), downloaded: 2_097_152, total: Some(9_437_184) })
                },
            ),
            (
                "downloading_unknown_size",
                base(Phase::Downloading { version: "0.1.1".into(), downloaded: 512, total: None }),
            ),
            (
                "ready",
                UpdateStatus {
                    notes: Some("修了几个 bug".into()),
                    last_update: Some(LastUpdate { from: "0.0.9".into(), to: "0.1.0".into(), at: 1_767_139_200_000 }),
                    ..base(Phase::Ready { version: "0.1.1".into() })
                },
            ),
            ("installing", base(Phase::Installing { version: "0.1.1".into() })),
            (
                "failed_check",
                UpdateStatus {
                    last_error: Some("连不上 GitHub（网络不通；注意：更新检查不读系统代理）".into()),
                    error_kind: Some(ErrorKind::Check),
                    ..base(Phase::Idle)
                },
            ),
            (
                "failed_install",
                UpdateStatus {
                    last_error: Some("安装中途失败，而且旧版本没能放回原位。旧版本在 /Applications/SENSE (previous version).app，请手动把它拖回「应用程序」文件夹".into()),
                    error_kind: Some(ErrorKind::Install),
                    ..base(Phase::Idle)
                },
            ),
            (
                "ready_install_failed",
                UpdateStatus {
                    notes: Some("修了几个 bug".into()),
                    last_error: Some("没能替换「应用程序」里的 SENSE：可能是你取消了管理员授权，也可能是当前账户没有权限（请用管理员账户登录，或手动安装新版本）".into()),
                    error_kind: Some(ErrorKind::Install),
                    ..base(Phase::Ready { version: "0.1.1".into() })
                },
            ),
            (
                "unavailable",
                UpdateStatus {
                    unavailable: Some("开发版（debug 构建）不检查更新".into()),
                    auto_update: false,
                    checked_at: None,
                    ..base(Phase::Idle)
                },
            ),
        ];
        let fixture: Value = serde_json::from_str(include_str!("../../src/lib/updateStatus.fixture.json"))
            .expect("夹具不是合法 JSON");
        let fixture = fixture.as_object().expect("夹具根节点必须是对象");
        assert_eq!(
            fixture.len(),
            cases.len(),
            "夹具里的条目数和这里的用例数对不上：{:?}",
            fixture.keys().collect::<Vec<_>>()
        );
        for (name, status) in cases {
            let expected = fixture.get(name).unwrap_or_else(|| panic!("夹具里缺 {name}"));
            assert_eq!(&serde_json::to_value(&status).unwrap(), expected, "{name}");
        }
    }

    // ---------- 发布配置不能留占位 / 测试值 ----------

    fn conf() -> Value {
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json 不是合法 JSON")
    }

    /// 公钥错了或是占位：已经发出去的安装包永远验不过任何更新，而且发现不了。必须在提交前拦住。
    #[test]
    fn config_pubkey_is_a_real_minisign_public_key() {
        use base64::Engine as _;
        let conf = conf();
        let pubkey = conf
            .pointer("/plugins/updater/pubkey")
            .and_then(Value::as_str)
            .expect("tauri.conf.json 缺 plugins.updater.pubkey");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(pubkey)
            .expect("pubkey 不是 base64——应当是 `tauri signer generate` 生成的 .pub 文件的内容");
        let text = String::from_utf8(decoded).expect("pubkey 解出来不是文本");
        minisign_verify::PublicKey::decode(&text)
            .expect("pubkey 解出来不是合法的 minisign 公钥（是不是还留着占位？）");
    }

    #[test]
    fn config_points_at_the_github_release_and_never_ships_test_overrides() {
        let conf = conf();
        let updater = conf.pointer("/plugins/updater").expect("缺 plugins.updater");
        assert_eq!(
            updater.get("endpoints"),
            Some(&json!(["https://github.com/cpcboy/TranslateApp/releases/latest/download/latest.json"]))
        );
        assert_eq!(updater.get("requireSignedVersion"), Some(&json!(true)), "版本号必须绑进签名，防回滚");
        // 本机端到端验证才会临时打开的开关：提交进去等于给所有人关掉 HTTPS 要求
        for key in [
            "dangerousInsecureTransportProtocol",
            "dangerousAcceptInvalidCerts",
            "dangerousAcceptInvalidHostnames",
            "allowDowngrades",
        ] {
            assert!(
                updater.get(key).is_none_or(|value| value == &json!(false)),
                "{key} 不能留在提交的配置里"
            );
        }
    }

    /// `Cargo.toml` 里更新插件那一行是不是还关着默认特性（纯函数，下面的用例喂各种写法）。
    /// 只认单行写法：找不到（比如改成了多行表格式）就当作不通过，让人来看一眼，而不是悄悄放过。
    fn updater_dependency_keeps_defaults_off(manifest: &str) -> Result<(), String> {
        let line = manifest
            .lines()
            .map(|line| line.split('#').next().unwrap_or("").trim())
            .find(|line| line.starts_with("tauri-plugin-updater"))
            .ok_or("找不到 `tauri-plugin-updater = { … }` 这一行（改成多行表格式了？这条守卫只认单行写法）")?;
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.contains("default-features=false") && !compact.contains("system-proxy") {
            Ok(())
        } else {
            Err(format!("更新插件必须 default-features = false 且不开 system-proxy：{line}"))
        }
    }

    #[test]
    fn the_updater_dependency_check_reads_the_line_the_way_cargo_does() {
        let ok = r#"tauri-plugin-updater = { version = "2", default-features = false, features = ["rustls-tls", "zip"] }"#;
        assert!(updater_dependency_keeps_defaults_off(ok).is_ok());
        // 没写空格也一样
        assert!(updater_dependency_keeps_defaults_off(&ok.replace("default-features = false", "default-features=false")).is_ok());
        // 行尾注释里写着 default-features = false、其实默认特性开着：不能被注释骗过去
        assert!(
            updater_dependency_keeps_defaults_off(r#"tauri-plugin-updater = { version = "2" } # default-features = false"#).is_err()
        );
        // 点名把 system-proxy 打开
        assert!(updater_dependency_keeps_defaults_off(
            r#"tauri-plugin-updater = { version = "2", default-features = false, features = ["system-proxy"] }"#
        )
        .is_err());
        // 回到最初的写法（默认特性全开）
        assert!(updater_dependency_keeps_defaults_off(r#"tauri-plugin-updater = "2.14.1""#).is_err());
        // 只有注释里提到它、没有真正的依赖行
        assert!(updater_dependency_keeps_defaults_off("# tauri-plugin-updater = { default-features = false }\n").is_err());
        // 多行表格式：认不出来，宁可报错
        assert!(updater_dependency_keeps_defaults_off("[dependencies.tauri-plugin-updater]\nversion = \"2\"\n").is_err());
    }

    /// 更新插件的默认特性带着 `system-proxy`：它打开 reqwest 的系统代理支持，而 reqwest 是整个进程共用的，
    /// 模型请求（查词、截屏翻译）也会跟着去读 macOS 系统代理。应用一直只认环境变量里的代理，不能因为加了更新器就变。
    #[test]
    fn updater_plugin_does_not_turn_on_the_system_proxy() {
        if let Err(why) = updater_dependency_keeps_defaults_off(include_str!("../Cargo.toml")) {
            panic!("{why}。默认特性里的 system-proxy 会让进程里所有 reqwest 客户端（包括模型请求）去读 macOS 系统代理");
        }
        // 依赖图里也不能出现：别的依赖把它重新打开，上面那行检查不出来（`-sys` 也算）
        assert!(
            !include_str!("../Cargo.lock").contains("name = \"system-configuration"),
            "Cargo.lock 里出现了 system-configuration：多半是 reqwest / hyper-util 的系统代理支持被打开了。\
             `cargo tree -i system-configuration` 看是谁拉进来的；如果是别的 crate 的正当需要，把这条守卫改成只查 hyper-util 的依赖"
        );
    }

    #[test]
    fn app_version_is_the_same_in_cargo_tauri_conf_and_package_json() {
        let package: Value = serde_json::from_str(include_str!("../../package.json")).unwrap();
        let cargo = env!("CARGO_PKG_VERSION");
        assert_eq!(conf().get("version").and_then(Value::as_str), Some(cargo), "tauri.conf.json 的 version 和 Cargo.toml 不一致");
        assert_eq!(package.get("version").and_then(Value::as_str), Some(cargo), "package.json 的 version 和 Cargo.toml 不一致");
    }

    // ---------- 重试节奏、说明归属、序号 ----------

    #[test]
    fn rejected_packages_back_off_to_the_normal_interval_network_failures_retry_soon() {
        let mut inner = Inner::default();
        inner.finish_check(1, Err(failure("取不到更新清单", false)));
        assert_eq!(inner.retry_wait_ms, RETRY_AFTER_FAILURE_MS, "网络类失败 30 分钟后重试");
        // 包下载下来了但验不过：多半是发布出了问题，30 分钟重下一次 5 MB 没有意义（一天 48 次）
        inner.finish_check(2, Err(failure("更新包的签名校验没通过", true)));
        assert_eq!(inner.retry_wait_ms, CHECK_INTERVAL_MS, "签名类失败退避到正常间隔");
        inner.finish_check(3, Ok(Outcome::UpToDate));
        assert_eq!(inner.retry_wait_ms, CHECK_INTERVAL_MS);
    }

    #[test]
    fn a_failed_newer_download_does_not_overwrite_the_notes_of_the_ready_version() {
        let mut inner = Inner::default();
        inner.start_download("0.1.1", Some("0.1.1 的说明".into()));
        inner.finish_check(1, Ok(ready("0.1.1", true)));
        assert_eq!(status_of(&inner).notes.as_deref(), Some("0.1.1 的说明"));

        // 0.1.2 开始下载：下载中显示它自己的说明
        inner.start_check();
        inner.start_download("0.1.2", Some("0.1.2 的说明".into()));
        assert_eq!(status_of(&inner).notes.as_deref(), Some("0.1.2 的说明"));

        // 下载失败：回到 0.1.1，说明必须还是 0.1.1 的，不能配着 0.1.2 的说明去点「立即重启」
        inner.finish_check(2, Err(failure("下载失败", false)));
        let status = status_of(&inner);
        assert_eq!(status.phase, Phase::Ready { version: "0.1.1".into() });
        assert_eq!(status.notes.as_deref(), Some("0.1.1 的说明"));

        // 复查发现 0.1.1 还在（没重新下载）：它的说明原样留着；下载成功换成 0.1.2：说明跟着换
        inner.finish_check(3, Ok(ready("0.1.1", false)));
        assert_eq!(status_of(&inner).notes.as_deref(), Some("0.1.1 的说明"));
        inner.start_download("0.1.2", Some("0.1.2 的说明".into()));
        inner.finish_check(4, Ok(ready("0.1.2", true)));
        assert_eq!(status_of(&inner).notes.as_deref(), Some("0.1.2 的说明"));
    }

    #[test]
    fn every_state_change_bumps_the_sequence_number() {
        // 前端靠序号丢掉晚到的旧状态：漏了 +1，新状态会被当成旧的丢掉
        let mut inner = Inner::default();
        assert_eq!(inner.seq, 0);
        inner.mutate(|inner| inner.start_check());
        assert_eq!(inner.seq, 1);
        inner.mutate(|inner| inner.finish_check(5, Ok(Outcome::UpToDate)));
        assert_eq!(inner.seq, 2);
        inner.mutate(|inner| inner.auto_update = false);
        assert_eq!(inner.status("0.1.0".into(), None).seq, 3);
    }

    // ---------- 小的纯函数 ----------

    #[test]
    fn auto_update_defaults_to_on_unless_explicitly_false() {
        assert!(parse_auto_update(None), "读不出设置文件：开");
        assert!(parse_auto_update(Some(&json!({ "hotkey": "ctrl+alt+k" }))), "没写：开");
        assert!(!parse_auto_update(Some(&json!({ "auto_update": false }))));
        assert!(parse_auto_update(Some(&json!({ "auto_update": true }))));
        // 手改成了字符串 / 数字：不认，按缺省（开），不当成「关」
        assert!(parse_auto_update(Some(&json!({ "auto_update": "false" }))));
        assert!(parse_auto_update(Some(&json!({ "auto_update": 0 }))));
    }

    #[test]
    fn download_url_must_come_from_the_same_origin_as_the_manifest() {
        let endpoint = "https://github.com/cpcboy/TranslateApp/releases/latest/download/latest.json";
        assert!(same_origin("https://github.com/cpcboy/TranslateApp/releases/download/v0.1.1/SENSE_0.1.1_aarch64.app.tar.gz", endpoint));
        // 清单被改成指向别处：协议、主机、端口任何一个不同都拒绝
        assert!(!same_origin("https://evil.example/SENSE.app.tar.gz", endpoint));
        assert!(!same_origin("http://github.com/x.tar.gz", endpoint), "降级成 http 也不行");
        assert!(!same_origin("https://github.com.evil.example/x.tar.gz", endpoint), "前缀相同的主机不算");
        assert!(!same_origin("https://github.com:8443/x.tar.gz", endpoint));
        assert!(!same_origin("not a url", endpoint));
        // 本机演练：清单和安装包都在 127.0.0.1:18765
        assert!(same_origin("http://127.0.0.1:18765/SENSE.app.tar.gz", "http://127.0.0.1:18765/latest.json"));
        assert!(!same_origin("http://127.0.0.1:18766/SENSE.app.tar.gz", "http://127.0.0.1:18765/latest.json"));
    }

    #[test]
    fn signature_failures_are_rejections_and_only_a_broken_rollback_blocks_retry() {
        use tauri_plugin_updater::Error as E;
        assert!(is_rejection(&E::MissingSignedVersion));
        // 最典型的一类：包被改过、或者不是这把密钥签的
        assert!(is_rejection(&E::Minisign(minisign_verify::Error::InvalidSignature)));
        assert!(is_rejection(&E::SignedVersionMismatch { signed: "0.1.1".into(), announced: "0.1.2".into() }));
        assert!(!is_rejection(&E::ReleaseNotFound), "清单取不到不是包被拒");
        assert!(!is_rejection(&E::Network("Download request failed with status: 404 Not Found".into())));
        assert!(install_error_is_retryable(&E::AuthenticationFailed));
        assert!(install_error_is_retryable(&E::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied))));
        assert!(!install_error_is_retryable(&E::PreviousAppNotRestored(PathBuf::from("/Applications/x (previous version).app"))));
    }

    #[test]
    fn download_url_from_another_origin_is_rejected_and_marked_as_a_publishing_problem() {
        let endpoint = "https://github.com/cpcboy/TranslateApp/releases/latest/download/latest.json";
        let ours = "https://github.com/cpcboy/TranslateApp/releases/download/v0.1.1/SENSE_0.1.1_aarch64.app.tar.gz";
        assert_eq!(check_download_origin(ours, Some(endpoint)), Ok(()));
        let failure = check_download_origin("https://evil.example/SENSE.app.tar.gz", Some(endpoint)).unwrap_err();
        assert!(failure.rejected, "拒收属于「发布出了问题」一类，退避到下一个正常间隔");
        assert!(failure.message.contains("没有下载任何东西"), "{}", failure.message);
        // 读不到配置里的端点：不校验
        assert_eq!(check_download_origin("https://evil.example/SENSE.app.tar.gz", None), Ok(()));
    }

    #[test]
    fn a_ready_outcome_without_a_stored_package_becomes_a_failure_right_away() {
        let ready = || Ok(Outcome::Ready { version: "0.1.1".to_string(), fresh: true });
        assert_eq!(reconcile(ready(), true), ready());
        let failure = reconcile(ready(), false).unwrap_err();
        assert!(failure.message.contains("内部错误"), "{}", failure.message);
        assert!(!failure.rejected, "这不是包被拒，下次照常 30 分钟后重试");
        // 其余结果原样放过：没有新版本时本来就没有包，失败就是失败
        assert_eq!(reconcile(Ok(Outcome::UpToDate), false), Ok(Outcome::UpToDate));
        let failed = || Err(CheckFailure { message: "连不上 GitHub".to_string(), rejected: false });
        assert_eq!(reconcile(failed(), false), failed());
    }

    #[test]
    fn clicking_restart_while_busy_gets_an_honest_answer() {
        let downloading = Busy::Downloading { version: "0.1.2".to_string(), downloaded: 1, total: None };
        assert_eq!(busy_reply(Some(&downloading)), Some("正在下载更新 v0.1.2，下载完再点".to_string()));
        let installing = Busy::Installing { version: "0.1.2".to_string() };
        assert_eq!(busy_reply(Some(&installing)), Some("正在安装，不用重复点".to_string()));
        assert_eq!(busy_reply(Some(&Busy::Checking)), None, "检查只要几秒钟，等它做完再装");
        assert_eq!(busy_reply(None), None);
    }

    // ---------- 重启脚本：真 /bin/sh + 假 opener ----------
    //
    // 假 opener 数自己被调了几次，到了指定的那一次才「起」一个新进程：`exec -a` 让它的 argv[0] 正好是
    // `<app>/Contents/MacOS/<名字>`，ps 里看起来就和真应用一样。旧进程用一个很快退出的 sleep 代替
    // （放在孙进程里，退出后由 launchd 回收；否则它会一直是测试进程的僵尸，`kill -0` 永远成功）。

    struct Relaunch {
        status: i32,
        opens: u32,
        log: String,
    }

    fn run_relaunch(
        name: &str,
        bundle_name: &str,
        starts_on_call: Option<u32>,
        child_name: &str,
        old_lives_secs: u32,
        polls: &str,
        wait_old: &str,
    ) -> Relaunch {
        let dir = std::env::temp_dir().join(format!("sense-relaunch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bundle = dir.join(bundle_name);
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let opens = dir.join("opens");
        let pids = dir.join("pids");
        let opener = dir.join("fake-open");
        let start_if = match starts_on_call {
            Some(n) => format!("[ \"$count\" -ge {n} ]"),
            None => "false".to_string(),
        };
        std::fs::write(
            &opener,
            format!(
                "#!/bin/bash\ncount=$(( $(cat '{opens}' 2>/dev/null || echo 0) + 1 ))\necho $count > '{opens}'\n\
                 if {start_if}; then\n  ( exec -a \"$2/Contents/MacOS/{child_name}\" /bin/sleep 20 ) >/dev/null 2>&1 &\n  echo $! >> '{pids}'\nfi\nexit 0\n",
                opens = opens.display(),
                pids = pids.display(),
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let spawned = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("/bin/sleep {old_lives_secs} >/dev/null 2>&1 & echo $!"))
            .output()
            .unwrap();
        let old_pid = String::from_utf8(spawned.stdout).unwrap().trim().to_string();

        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(RELAUNCH_SCRIPT)
            .arg("sense-relaunch")
            .arg(&old_pid)
            .arg(&bundle)
            .arg(&opener)
            .arg(polls)
            .arg(wait_old)
            .env("HOME", &home)
            .output()
            .unwrap();

        let result = Relaunch {
            status: output.status.code().unwrap_or(-1),
            opens: std::fs::read_to_string(&opens)
                .ok()
                .and_then(|text| text.trim().parse().ok())
                .unwrap_or(0),
            log: std::fs::read_to_string(home.join("Library/Logs/SENSE-update.log")).unwrap_or_default(),
        };
        // 收拾：假「新进程」和还没退出的「旧进程」
        for pid in std::fs::read_to_string(&pids).unwrap_or_default().lines() {
            let _ = Command::new("/bin/kill").arg(pid.trim()).output();
        }
        let _ = Command::new("/bin/kill").arg(&old_pid).output();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn relaunch_waits_for_the_old_process_then_opens_once_when_the_new_one_comes_up() {
        let result = run_relaunch("first", "Fake.app", Some(1), "sense", 1, "10", "50");
        assert_eq!(result.status, 0);
        assert_eq!(result.opens, 1, "第一次就起来了，不能再多开");
        assert_eq!(result.log, "", "成功了不写日志");
    }

    #[test]
    fn relaunch_retries_when_the_new_process_does_not_appear() {
        // 第二次调用 opener 才真的起来：第一次白等（3 × 0.2 秒），再开一次
        let result = run_relaunch("retry", "Fake.app", Some(2), "sense", 1, "3", "50");
        assert_eq!(result.status, 0);
        assert_eq!(result.opens, 2);
    }

    #[test]
    fn relaunch_gives_up_after_three_attempts_and_leaves_a_log_line() {
        let result = run_relaunch("giveup", "Fake.app", None, "sense", 1, "3", "50");
        assert_eq!(result.status, 1);
        assert_eq!(result.opens, 3, "最多 3 次，不能无限开");
        assert!(result.log.contains("open -n 试了 3 次") && result.log.contains("Fake.app"), "{}", result.log);
    }

    #[test]
    fn relaunch_does_not_open_a_second_instance_when_the_old_process_never_exits() {
        // 旧进程活 5 秒，只肯等 0.5 秒：不能去开新的（会和卡住的那个抢快捷键，还可能把它误认成新进程）
        let result = run_relaunch("hang", "Fake.app", Some(1), "sense", 5, "10", "5");
        assert_eq!(result.status, 1);
        assert_eq!(result.opens, 0);
        assert!(result.log.contains("没有退出"), "{}", result.log);
    }

    #[test]
    fn relaunch_does_not_count_the_ocr_helper_as_the_app_coming_up() {
        // 只有 sense-ocr 这个子进程在（命令行也以 <app>/Contents/MacOS/ 开头）：不算应用起来了
        let result = run_relaunch("ocr", "Fake.app", Some(1), "sense-ocr", 1, "3", "50");
        assert_eq!(result.status, 1);
        assert_eq!(result.opens, 3);
    }

    #[test]
    fn relaunch_survives_awkward_bundle_paths() {
        // 空格、引号、$、反斜杠（含字面量 \n）、括号：路径只当单个参数传，ENVIRON 交给 awk，不经过 -v 的转义处理
        let result = run_relaunch("weird", "My \"App\" $HOME back\\nslash (1).app", Some(1), "sense", 1, "10", "50");
        assert_eq!(result.status, 0, "{}", result.log);
        assert_eq!(result.opens, 1, "路径里有反斜杠时检测不能永远匹配不上，否则会连开三个实例");
    }
}
