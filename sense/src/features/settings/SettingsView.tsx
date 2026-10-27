import { useCallback, useEffect, useRef, useState } from "react";
import { api, type HotkeyView, type ProviderView } from "../../lib/ipc";
import { isUnsavedForm } from "./form";

export function SettingsView({ onBack }: { onBack: () => void }) {
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("");
  const [keyDirty, setKeyDirty] = useState(false);
  // 明文 Key 只经过 DOM 节点，不进 React state / 任何可序列化的 store。
  const keyRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [hotkeys, setHotkeys] = useState<HotkeyView[]>([]);
  const [capturing, setCapturing] = useState<string | null>(null);
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await api.listProviders();
      setProviders(list);
      setSelectedId((current) => current ?? list.find((item) => item.active)?.id ?? list[0]?.id ?? null);
    } catch (err) {
      setNotice({ ok: false, text: String(err) });
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    void api
      .hotkeyList()
      .then(setHotkeys)
      .catch(() => {
        /* 取不到就不显示这一栏 */
      });
  }, []);

  // 改键：按下的下一个组合直接生效（Esc 取消）
  useEffect(() => {
    if (!capturing) return;
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setCapturing(null);
        setHotkeyError(null);
        return;
      }
      const parsed = comboFrom(event);
      if ("error" in parsed) {
        setHotkeyError(parsed.error);
        return;
      }
      void api
        .setHotkey(capturing, parsed.spec)
        .then((view) => {
          setHotkeys((list) => list.map((item) => (item.kind === view.kind ? view : item)));
          setCapturing(null);
          setHotkeyError(null);
        })
        .catch((err) => {
          setHotkeyError(String(err).replace(/^Error:\s*/, ""));
          // 失败时 Rust 可能已经装不回旧键（它会把旧键忘掉）：重新取一次，行上才会标「未生效」
          void api
            .hotkeyList()
            .then(setHotkeys)
            .catch(() => {});
        });
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [capturing]);

  const selected = providers.find((item) => item.id === selectedId) ?? null;

  useEffect(() => {
    if (!selected) return;
    setBaseUrl(selected.base_url);
    setModel(selected.model);
    if (keyRef.current) keyRef.current.value = "";
    setKeyDirty(false);
    setNotice(null);
    // 只在切换 provider 时重置表单
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected?.id]);

  async function guard(label: string, task: () => Promise<void>) {
    setBusy(label);
    setNotice(null);
    try {
      await task();
    } catch (err) {
      setNotice({ ok: false, text: String(err) });
    } finally {
      setBusy(null);
    }
  }

  if (!selected) {
    return <p className="p-6 text-[16px] text-muted">读取配置中…</p>;
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex flex-wrap gap-2">
        {providers.map((provider) => (
          <button
            key={provider.id}
            type="button"
            onClick={() => setSelectedId(provider.id)}
            className={
              provider.id === selected.id
                ? "rounded-full border border-acc/60 bg-[rgba(255,82,48,0.12)] px-4 py-2.5 text-[15px] text-paper"
                : "rounded-full border border-line px-4 py-2.5 text-[15px] text-muted hover:border-line-strong hover:text-paper"
            }
          >
            {provider.label}
            {provider.active && <span className="ml-1 text-acc">●</span>}
          </button>
        ))}
      </div>

      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto pr-0.5">
        <Field label="Base URL">
          <input
            value={baseUrl}
            onChange={(event) => setBaseUrl(event.currentTarget.value)}
            spellCheck={false}
            className="w-full bg-transparent font-mono text-[16px] text-paper focus:outline-none"
          />
        </Field>

        <Field label="模型">
          <input
            value={model}
            onChange={(event) => setModel(event.currentTarget.value)}
            spellCheck={false}
            className="w-full bg-transparent font-mono text-[16px] text-paper focus:outline-none"
          />
        </Field>

        <div className="rounded-lg border border-line px-3.5 py-3">
          <div className="flex items-center justify-between">
            <span className="text-[13.5px] tracking-[0.08em] text-muted">API Key</span>
            {selected.key_masked ? (
              <span className="font-mono text-[14px] text-muted">{selected.key_masked}</span>
            ) : (
              <span className="text-[14px] text-muted">
                {selected.requires_key ? "未设置" : "本地模型，无需 Key"}
              </span>
            )}
          </div>

          <div className="mt-2 flex items-center gap-2">
            <div className="relative min-w-0 flex-1">
              <input
                ref={keyRef}
                type="password"
                onChange={(event) => setKeyDirty(event.currentTarget.value.trim().length > 0)}
                autoComplete="off"
                spellCheck={false}
                aria-label="API Key"
                className="w-full min-w-0 bg-transparent font-mono text-[16px] text-paper focus:outline-none"
              />
              {/* 自绘占位符：WKWebView 里 input 的 placeholder 渲染不出中文 */}
              {!keyDirty && (
                <span aria-hidden="true" className="pointer-events-none absolute inset-y-0 left-0 flex items-center font-mono text-[16px] text-subtle">
                  {selected.requires_key ? "粘贴 Key" : "本地模型可留空"}
                </span>
              )}
            </div>
            <button
              type="button"
              disabled={!keyDirty || busy !== null}
              onClick={() =>
                guard("key", async () => {
                  const value = keyRef.current?.value.trim() ?? "";
                  if (!value) {
                    setNotice({ ok: false, text: "Key 不能为空" });
                    return;
                  }
                  const mask = await api.setApiKey(selected.id, value);
                  if (keyRef.current) keyRef.current.value = "";
                  setKeyDirty(false);
                  await refresh();
                  setNotice({ ok: true, text: `已存进钥匙串，设置页只显示 ${mask}` });
                })
              }
              className="shrink-0 rounded-md border border-line-strong px-3.5 py-2.5 text-[14.5px] text-paper disabled:opacity-60"
            >
              保存 Key
            </button>
            {selected.key_masked && (
              <button
                type="button"
                disabled={busy !== null}
                onClick={() =>
                  guard("clear", async () => {
                    await api.clearApiKey(selected.id);
                    await refresh();
                    setNotice({ ok: true, text: "已从钥匙串删除" });
                  })
                }
                className="shrink-0 rounded-md px-3 py-2.5 text-[14.5px] text-muted hover:text-acc"
              >
                删除
              </button>
            )}
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            disabled={busy !== null}
            onClick={() =>
              guard("save", async () => {
                await api.saveProvider(selected.id, baseUrl, model);
                await refresh();
                setNotice({ ok: true, text: "配置已保存" });
              })
            }
            className="rounded-md border border-line-strong px-4 py-2.5 text-[14.5px] text-paper disabled:opacity-60"
          >
            保存配置
          </button>

          <button
            type="button"
            disabled={busy !== null}
            onClick={() =>
              guard("test", async () => {
                // 明文 Key 只在这一刻从输入框读出来、交给 Rust 用一次；不进 state，也不保存
                const typedKey = keyRef.current?.value.trim() ?? "";
                const result = await api.testConnection(selected.id, { baseUrl, model, key: typedKey });
                // 测的是表单里的值：和已保存的不一样时要说清，免得用户以为测的是生效的那份
                const unsaved = isUnsavedForm(selected, { baseUrl, model, key: typedKey });
                setNotice({
                  ok: result.ok,
                  text:
                    (result.ok
                      ? `连接正常 · ${result.model} · ${result.latency_ms}ms`
                      : result.message) + (unsaved ? "（测的是表单里还没保存的值）" : ""),
                });
              })
            }
            className="rounded-md bg-acc-strong px-4 py-2.5 text-[14.5px] font-semibold text-[#0a0a0b] disabled:opacity-60"
          >
            {busy === "test" ? "测试中…" : "测试连接"}
          </button>

          {!selected.active && (
            <button
              type="button"
              disabled={busy !== null}
              onClick={() =>
                guard("active", async () => {
                  await api.setActiveProvider(selected.id);
                  await refresh();
                  setNotice({ ok: true, text: `已切换到 ${selected.label}` });
                })
              }
              className="rounded-md px-4 py-2.5 text-[14.5px] text-muted hover:text-paper"
            >
              设为当前
            </button>
          )}

          <button
            type="button"
            onClick={onBack}
            className="ml-auto rounded-md px-3.5 py-2.5 text-[14.5px] text-muted hover:text-paper"
          >
            返回查词
          </button>
        </div>

        {notice && (
          <p
            className={
              notice.ok
                ? "rounded-md bg-[rgba(108,138,90,0.14)] px-4 py-3 text-[14.5px] leading-relaxed text-[#b8d3a9]"
                : "rounded-md bg-[rgba(255,82,48,0.12)] px-4 py-3 text-[14.5px] leading-relaxed text-[#f6b09b]"
            }
          >
            {notice.text}
          </p>
        )}

        <div className="rounded-lg border border-line px-4 py-3">
          <div className="text-[13.5px] tracking-[0.08em] text-muted">快捷键</div>
          {hotkeys.map((item) => (
            <div key={item.kind} className="mt-2 flex items-center justify-between gap-3">
              <span className="text-[14.5px] text-paper">
                {item.kind === "lookup" ? "查词" : "截屏翻译"}
              </span>
              <div className="flex items-center gap-2">
                <kbd
                  className={
                    capturing === item.kind
                      ? "rounded border border-acc/60 bg-[rgba(255,82,48,0.12)] px-2.5 py-1 font-mono text-[13.5px] text-paper"
                      : "rounded border border-line px-2.5 py-1 font-mono text-[13.5px] text-muted"
                  }
                >
                  {capturing === item.kind ? "按下新组合…" : item.display}
                </kbd>
                {!item.active && capturing !== item.kind && (
                  <span className="text-[13px] text-[#f6b09b]">未生效 · 多半被别的 App 占了</span>
                )}
                <button
                  type="button"
                  onClick={() => {
                    setHotkeyError(null);
                    setCapturing(capturing === item.kind ? null : item.kind);
                  }}
                  className="rounded-md px-2.5 py-1 text-[13px] text-muted hover:text-paper"
                >
                  {capturing === item.kind ? "取消" : "修改"}
                </button>
              </div>
            </div>
          ))}
          {hotkeyError && (
            <p className="mt-2 text-[13px] leading-relaxed text-[#f6b09b]">{hotkeyError}</p>
          )}
          <p className="mt-2 text-[13px] leading-relaxed text-subtle">
            至少包含 ⌃ 或 ⌘，主键支持字母、数字、空格。改完立刻生效，不用重启。
          </p>
        </div>

        <p className="text-[13.5px] leading-relaxed text-subtle">
          Key 只写进 macOS 钥匙串，不会出现在界面、日志或配置文件里。
          所有模型请求由 Rust 发起，前端拿不到 Key。
        </p>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block rounded-lg border border-line px-4 py-3">
      <span className="block text-[13.5px] tracking-[0.08em] text-muted">{label}</span>
      <div className="mt-1.5">{children}</div>
    </label>
  );
}

/** 键盘事件 → 配置里那种 `ctrl+alt+cmd+x`；不合规就返回原因。 */
function comboFrom(event: KeyboardEvent): { spec: string } | { error: string } {
  if (["Control", "Alt", "Shift", "Meta"].includes(event.key)) {
    return { error: "还差一个主键：按住修饰键再按字母" };
  }

  const modifiers: string[] = [];
  if (event.ctrlKey) modifiers.push("ctrl");
  if (event.altKey) modifiers.push("alt");
  if (event.shiftKey) modifiers.push("shift");
  if (event.metaKey) modifiers.push("cmd");

  if (!modifiers.includes("ctrl") && !modifiers.includes("cmd")) {
    return { error: "至少要有 ⌃ 或 ⌘，否则会抢走所有输入框的按键" };
  }

  let main = "";
  if (event.key === " ") main = "space";
  else if (/^[a-z]$/i.test(event.key)) main = event.key.toLowerCase();
  else if (/^[0-9]$/.test(event.key)) main = event.key;
  else return { error: "只支持字母、数字、空格" };

  return { spec: [...modifiers, main].join("+") };
}
