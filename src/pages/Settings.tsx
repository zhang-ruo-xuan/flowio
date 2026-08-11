import { useState, useEffect, useCallback, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import {
  Moon, Sun, Zap, Keyboard, Palette, ArrowLeft, Eye, EyeOff,
  Star, Trash2, Check, Plus,
} from "lucide-react";

interface SettingsPageProps {
  darkMode: boolean;
  onToggleDark: () => void;
}

interface ProviderDef {
  id: string;
  name: string;
  base_url: string;
  models: string[];
  custom?: boolean;
}

interface ProviderConfig {
  model: string;
  apiKey: string;
  configured: boolean;
}

const BUILTIN_PROVIDERS: ProviderDef[] = [
  { id: "zhipu", name: "智谱 AI", base_url: "https://open.bigmodel.cn/api/paas/v4/chat/completions", models: ["glm-4v"] },
  { id: "qwen", name: "阿里百炼 (通义千问)", base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1", models: ["qwen-vl-plus", "qwen-vl-max"] },
  { id: "doubao", name: "字节豆包", base_url: "https://ark.cn-beijing.volces.com/api/v3", models: ["doubao-vision-pro-32k"] },
  { id: "yi", name: "零一万物 Yi", base_url: "https://api.lingyiwanwu.com/v1", models: ["yi-vision"] },
  { id: "openai", name: "OpenAI", base_url: "https://api.openai.com/v1", models: ["gpt-4o", "gpt-4o-mini", "gpt-4-turbo"] },
  { id: "anthropic", name: "Anthropic Claude", base_url: "https://api.anthropic.com/v1", models: ["claude-3.5-sonnet", "claude-3-opus", "claude-3-haiku"] },
  { id: "gemini", name: "Google Gemini", base_url: "https://generativelanguage.googleapis.com/v1beta", models: ["gemini-1.5-pro", "gemini-1.5-flash"] },
];

const CUSTOM_PROVIDERS_KEY = "custom_providers";

export default function SettingsPage({ darkMode, onToggleDark }: SettingsPageProps) {
  const navigate = useNavigate();
  const [activeTab, setActiveTab] = useState<"ai" | "shortcuts" | "appearance">("ai");

  /* ---- AI Config State ---- */
  const [defaultProvider, setDefaultProvider] = useState("zhipu");
  const [providers, setProviders] = useState<ProviderDef[]>([...BUILTIN_PROVIDERS]);
  const providersRef = useRef(providers);
  const [selectedId, setSelectedId] = useState("zhipu");
  const [providerConfigs, setProviderConfigs] = useState<Record<string, ProviderConfig>>({});
  const [loadingConfigs, setLoadingConfigs] = useState(true);

  // Editing state for the currently selected provider
  const [editApiKey, setEditApiKey] = useState("");
  const [editModel, setEditModel] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveResult, setSaveResult] = useState<string | null>(null);

  // Custom provider inline form state
  const [customName, setCustomName] = useState("");
  const [customBaseUrl, setCustomBaseUrl] = useState("");
  const [customModels, setCustomModels] = useState("");
  const [customApiKey, setCustomApiKey] = useState("");
  const [showCustomKey, setShowCustomKey] = useState(false);
  const [customSaving, setCustomSaving] = useState(false);
  const [customSaveResult, setCustomSaveResult] = useState<string | null>(null);

  // Load default provider + all configs + custom providers on mount
  const loadAll = useCallback(async () => {
    setLoadingConfigs(true);
    try {
      const defaultId = await invoke<string>("get_default_provider");
      setDefaultProvider(defaultId);
      setSelectedId(defaultId);

      // Load custom providers from DB
      let customProviders: ProviderDef[] = [];
      try {
        customProviders = (await invoke<ProviderDef[]>("list_custom_providers")).map(p => ({ ...p, custom: true as const }));
      } catch { /* none */ }

      const allProviders = [...BUILTIN_PROVIDERS, ...customProviders];
      setProviders(allProviders);

      // Load configs for all providers
      const configs: Record<string, ProviderConfig> = {};
      for (const p of allProviders) {
        try {
          const cfg = await invoke<{ provider: { id: string; model: string }; api_key: string } | null>(
            "get_ai_config", { providerId: p.id }
          );
          if (cfg) {
            configs[p.id] = { model: cfg.provider.model, apiKey: cfg.api_key, configured: true };
          } else {
            configs[p.id] = { model: "", apiKey: "", configured: false };
          }
        } catch {
          configs[p.id] = { model: "", apiKey: "", configured: false };
        }
      }
      setProviderConfigs(configs);
    } catch { /* fallback */ } finally {
      setLoadingConfigs(false);
    }
  }, []);

  useEffect(() => { loadAll(); }, [loadAll]);

  // Sync editing fields when selected provider changes
  useEffect(() => {
    const cfg = providerConfigs[selectedId];
    setEditApiKey(cfg?.apiKey || "");
    const def = providers.find((p) => p.id === selectedId);
    setEditModel(cfg?.model || def?.models[0] || "");
    setShowKey(false);
    setSaveResult(null);
  }, [selectedId, providerConfigs, providers]);

  // Set default provider
  const handleSetDefault = async () => {
    try {
      await invoke("set_default_provider", { providerId: selectedId });
      setDefaultProvider(selectedId);
    } catch (e) { console.error(e); }
  };

  // Save config — validates first
  const handleSave = async () => {
    if (!editApiKey.trim()) { setSaveResult("请先输入 API Key"); return; }
    setSaving(true);
    setSaveResult(null);

    try {
      await invoke<string>("test_api_key", { providerId: selectedId, apiKey: editApiKey });
    } catch (e: unknown) {
      setSaveResult(typeof e === "string" ? e : String(e));
      setSaving(false);
      return;
    }

    const def = providers.find((p) => p.id === selectedId)!;
    try {
      await invoke("set_ai_config", {
        config: {
          provider: { id: def.id, name: def.name, base_url: def.base_url, model: editModel || def.models[0] },
          api_key: editApiKey,
        },
      });
      setSaveResult("保存成功");
      setProviderConfigs((prev) => ({
        ...prev,
        [selectedId]: { model: editModel || def.models[0], apiKey: editApiKey, configured: true },
      }));
    } catch (e: unknown) {
      setSaveResult(`保存失败: ${String(e)}`);
    } finally { setSaving(false); }
  };

  // Delete config for the current provider
  const handleDeleteConfig = async () => {
    try {
      await invoke("remove_ai_config", { providerId: selectedId });
      setEditApiKey("");
      setEditModel("");
      setSaveResult("配置已删除");
      setProviderConfigs((prev) => ({
        ...prev,
        [selectedId]: { model: "", apiKey: "", configured: false },
      }));
    } catch (e: unknown) {
      setSaveResult(`删除失败: ${String(e)}`);
    }
  };

  // Add custom provider
  const handleAddCustom = async () => {
    if (!customName.trim() || !customBaseUrl.trim() || !customApiKey.trim()) {
      setCustomSaveResult("请填写完整的服务商信息");
      return;
    }
    if (!/^https?:\/\//.test(customBaseUrl.trim())) {
      setCustomSaveResult("请输入正确的 API 地址，需以 http:// 或 https:// 开头");
      return;
    }
    if (providersRef.current.some((p) => p.name.toLowerCase() === customName.trim().toLowerCase())) {
      setCustomSaveResult("服务商名称已存在，请使用不同的名称");
      return;
    }
    setCustomSaving(true);
    setCustomSaveResult(null);

    // Validate API key before saving
    try {
      await invoke("validate_custom_api", {
        baseUrl: customBaseUrl.trim(),
        apiKey: customApiKey.trim(),
      });
    } catch (e: unknown) {
      setCustomSaveResult(`API 验证失败: ${String(e)}`);
      setCustomSaving(false);
      return;
    }

    const id = "custom_" + Date.now();
    const modelList = ["default"];
    const newProv: ProviderDef = { id, name: customName, base_url: customBaseUrl, models: modelList, custom: true };

    try {
      await invoke("add_custom_provider", {
        provider: { id, name: customName, base_url: customBaseUrl, models: modelList },
        apiKey: customApiKey,
      });
      setProviders((prev) => [...prev, newProv]);
      setProviderConfigs((prev) => ({ ...prev, [id]: { model: "", apiKey: customApiKey, configured: !!customApiKey } }));
      setCustomSaveResult("保存成功");
      setCustomName("");
      setCustomBaseUrl("");
      setCustomModels("");
      setCustomApiKey("");
    } catch (e: unknown) {
      setCustomSaveResult(`保存失败: ${String(e)}`);
    } finally {
      setCustomSaving(false);
    }
  };

  // Remove custom provider
  const handleRemoveCustom = async (pid: string) => {
    try {
      await invoke("remove_custom_provider", { providerId: pid });
    } catch (e) { console.error(e); }

    setProviders((prev) => prev.filter((p) => p.id !== pid));
    setProviderConfigs((prev) => {
      const next = { ...prev };
      delete next[pid];
      return next;
    });
    if (selectedId === pid) {
      setSelectedId(defaultProvider);
    }
  };

  const selectedProv = providers.find((p) => p.id === selectedId);
  const selectedCfg = providerConfigs[selectedId];
  const isDefault = defaultProvider === selectedId;

  /* ---- Shortcuts State ---- */
  const [shortcuts] = useState({ record: "Ctrl+Shift+R", pause: "Ctrl+Shift+P", stop: "Ctrl+Shift+S" });
  const [fontSize, setFontSize] = useState(() => localStorage.getItem("fontSize") || "14");
  useEffect(() => {
    document.body.style.zoom = `${parseInt(fontSize) / 14 * 100}%`;
  }, [fontSize]);

  providersRef.current = providers;

  return (
    <div className="page">
      {/* 返回栏 */}
      <div style={{ display: "flex", alignItems: "center", padding: "8px 0" }}>
        <button onClick={() => navigate("/")} style={{ display: "inline-flex", alignItems: "center", gap: 4, background: "transparent", border: "none", cursor: "pointer", padding: "6px 12px", borderRadius: 6, fontSize: 14, color: "var(--blue)" }}
          onMouseEnter={(e) => (e.currentTarget.style.background = "rgba(0,0,0,0.06)")} onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
          <ArrowLeft size={16} />返回
        </button>
      </div>

      <div className="page-header"><h1>设置</h1></div>

      {/* Tabs */}
      <div className="tabs">
        <button className={`tab ${activeTab === "ai" ? "active" : ""}`} onClick={() => setActiveTab("ai")}>
          <Zap size={14} style={{ marginRight: 4, verticalAlign: "middle" }} />AI 配置
        </button>
        <button className={`tab ${activeTab === "shortcuts" ? "active" : ""}`} onClick={() => setActiveTab("shortcuts")}>
          <Keyboard size={14} style={{ marginRight: 4, verticalAlign: "middle" }} />快捷键
        </button>
        <button className={`tab ${activeTab === "appearance" ? "active" : ""}`} onClick={() => setActiveTab("appearance")}>
          <Palette size={14} style={{ marginRight: 4, verticalAlign: "middle" }} />外观
        </button>
      </div>

      <div className="content-area" style={{ marginTop: 16 }}>
        {/* ── AI 配置 ── */}
        {activeTab === "ai" && (
          <div>
            {loadingConfigs ? (
              <div style={{ padding: "24px 0", color: "var(--text-secondary)" }}>加载配置中...</div>
            ) : (
              <>
                {/* Provider selector row */}
                <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 16 }}>
                  <select
                    value={selectedId}
                    onChange={(e) => setSelectedId(e.target.value)}
                    style={{ flex: 1 }}
                  >
                    {providers.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name} {providerConfigs[p.id]?.configured ? " ✓" : ""}
                      </option>
                    ))}
                    <option value="__custom__">-- 自定义服务商 --</option>
                  </select>

                  {isDefault && selectedId !== "__custom__" ? (
                    <span style={{ fontSize: 13, color: "var(--text-tertiary)", display: "inline-flex", alignItems: "center", gap: 4 }}>
                      <Star size={13} style={{ color: "#f5a623" }} />默认
                    </span>
                  ) : selectedId !== "__custom__" ? (
                    <button onClick={handleSetDefault} style={{ fontSize: 12, padding: "4px 10px", border: "1px solid var(--border)", borderRadius: 4, background: "transparent", cursor: "pointer", color: "var(--text-secondary)" }}>
                      设为默认
                    </button>
                  ) : null}

                  {selectedProv?.custom && (
                    <button onClick={() => handleRemoveCustom(selectedId)} style={{ fontSize: 12, padding: "4px 8px", border: "1px solid var(--red)", borderRadius: 4, background: "transparent", cursor: "pointer", color: "var(--red)", display: "inline-flex", alignItems: "center", gap: 3 }}>
                      <Trash2 size={13} />删除
                    </button>
                  )}
                </div>

                {/* Custom provider list */}
                {selectedProv?.custom && (
                  <div style={{ marginBottom: 16, border: "1px solid var(--border)", borderRadius: 8, padding: "10px 14px", background: "var(--bg-secondary)" }}>
                    <div style={{ fontSize: 12, color: "var(--text-secondary)", marginBottom: 8 }}>自定义服务商</div>
                    {providers.filter(p => p.custom).map((p) => (
                      <div key={p.id} style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "4px 0", fontSize: 13 }}>
                        <span>{p.name}</span>
                        <button onClick={() => handleRemoveCustom(p.id)} style={{ display: "inline-flex", alignItems: "center", gap: 2, background: "transparent", border: "none", cursor: "pointer", color: "var(--red)", fontSize: 12 }}>
                          <Trash2 size={12} />
                        </button>
                      </div>
                    ))}
                  </div>
                )}

                {/* Custom provider inline form */}
                {selectedId === "__custom__" ? (
                  <div style={{ border: "1px solid var(--border)", borderRadius: 8, overflow: "hidden" }}>
                    {/* Header */}
                    <div style={{ padding: "10px 16px", background: "var(--bg-secondary)", borderBottom: "1px solid var(--border)", fontSize: 14, fontWeight: 600, display: "flex", alignItems: "center", gap: 6 }}>
                      <Plus size={14} style={{ color: "var(--blue)" }} /> 添加自定义服务商
                    </div>
                    <div style={{ padding: "14px 16px" }}>
                      <div className="form-group" style={{ marginBottom: 10 }}>
                        <label>服务商名称</label>
                        <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                          <input value={customName} onChange={(e) => setCustomName(e.target.value)} placeholder="例如: 通义千问" style={{ flex: 1 }} />
                        </div>
                      </div>
                      <div className="form-group" style={{ marginBottom: 10 }}>
                        <label>API 地址 (base URL)</label>
                        <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                          <input value={customBaseUrl} onChange={(e) => setCustomBaseUrl(e.target.value)} placeholder="https://dashscope.aliyuncs.com/compatible-mode/v1" style={{ flex: 1 }} />
                        </div>
                      </div>
                      <div className="form-group" style={{ marginBottom: 10 }}>
                        <label>API Key</label>
                        <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                          <input type={showCustomKey ? "text" : "password"} placeholder="输入 API Key" value={customApiKey}
                            onChange={(e) => setCustomApiKey(e.target.value)} style={{ flex: 1 }} />
                          <button onClick={() => setShowCustomKey(!showCustomKey)} style={{ background: "transparent", border: "1px solid var(--border)", borderRadius: 6, padding: "6px 8px", cursor: "pointer", display: "inline-flex", alignItems: "center", color: "var(--text-secondary)" }}>
                            {showCustomKey ? <EyeOff size={16} /> : <Eye size={16} />}
                          </button>
                        </div>
                        <p className="hint">密钥将加密存储在本地，仅用于 AI 功能调用</p>
                      </div>
                      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                        <button onClick={handleAddCustom} disabled={customSaving}
                          className="btn btn-primary"
                          style={{ background: "var(--green)", fontSize: 13 }}>
                          {customSaving ? "保存中..." : "保存配置"}
                        </button>
                        {customSaveResult && <span style={{ fontSize: 13, color: customSaveResult === "保存成功" ? "var(--green)" : "var(--red)" }}>{customSaveResult}</span>}
                      </div>
                    </div>
                  </div>
                ) : (
                  /* Config form for existing provider */
                  selectedProv && (
                    <div style={{ border: "1px solid var(--border)", borderRadius: 8, padding: "14px 16px" }}>
                      <div className="form-group" style={{ marginBottom: 12 }}>
                        <label>API Key</label>
                        <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                          <input type={showKey ? "text" : "password"} placeholder="输入 API Key" value={editApiKey}
                            onChange={(e) => setEditApiKey(e.target.value)} style={{ flex: 1 }} />
                          <button onClick={() => setShowKey(!showKey)} style={{ background: "transparent", border: "1px solid var(--border)", borderRadius: 6, padding: "6px 8px", cursor: "pointer", display: "inline-flex", alignItems: "center", color: "var(--text-secondary)" }}>
                            {showKey ? <EyeOff size={16} /> : <Eye size={16} />}
                          </button>
                        </div>
                        <p className="hint">密钥将加密存储在本地，仅用于 AI 功能调用</p>
                      </div>

                      {!selectedProv?.custom && (
                      <div className="form-group" style={{ marginBottom: 14 }}>
                        <label>模型选择</label>
                        <select value={editModel} onChange={(e) => setEditModel(e.target.value)}>
                          {selectedProv.models.map((m) => (<option key={m} value={m}>{m}</option>))}
                        </select>
                      </div>
                      )}

                      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                        <button className="btn btn-primary" onClick={handleSave} disabled={saving} style={{ background: "var(--green)", fontSize: 13 }}>
                          {saving ? "保存中..." : "保存配置"}
                        </button>
                        {providerConfigs[selectedId]?.configured && (
                          <button onClick={handleDeleteConfig} style={{ display: "inline-flex", alignItems: "center", gap: 4, background: "transparent", border: "1px solid var(--red)", borderRadius: 6, padding: "6px 12px", cursor: "pointer", color: "var(--red)", fontSize: 13 }}>
                            <Trash2 size={14} />删除配置
                          </button>
                        )}
                        {saveResult && <span style={{ fontSize: 13, color: saveResult === "保存成功" ? "var(--green)" : "var(--red)" }}>{saveResult}</span>}
                      </div>
                    </div>
                  )
                )}
              </>
            )}
          </div>
        )}

        {/* ── 快捷键 ── */}
        {activeTab === "shortcuts" && (
          <div>
            <table className="table">
              <thead><tr><th>操作</th><th>快捷键</th></tr></thead>
              <tbody>
                <tr><td>开始 / 恢复录制</td><td><code style={{ background: "var(--bg-secondary)", padding: "2px 8px", borderRadius: 4, fontSize: 13 }}>{shortcuts.record}</code></td></tr>
                <tr><td>暂停录制</td><td><code style={{ background: "var(--bg-secondary)", padding: "2px 8px", borderRadius: 4, fontSize: 13 }}>{shortcuts.pause}</code></td></tr>
                <tr><td>停止录制</td><td><code style={{ background: "var(--bg-secondary)", padding: "2px 8px", borderRadius: 4, fontSize: 13 }}>{shortcuts.stop}</code></td></tr>
              </tbody>
            </table>
          </div>
        )}

        {/* ── 外观 ── */}
        {activeTab === "appearance" && (
          <div>
            <div className="form-group">
              <label>暗色模式</label>
              <div className="toggle-switch" onClick={onToggleDark}>
                <div className={`toggle-switch-track${darkMode ? " active" : ""}`}>
                  <div className="toggle-switch-thumb" />
                </div>
                <span className="toggle-switch-label">
                  {darkMode ? (<><Moon size={14} style={{ marginRight: 4, verticalAlign: "middle" }} />暗色模式</>) : (<><Sun size={14} style={{ marginRight: 4, verticalAlign: "middle" }} />日间模式</>)}
                </span>
              </div>
            </div>
            <div className="form-group">
              <label>字体大小</label>
              <select value={fontSize} onChange={(e) => { const v = e.target.value; setFontSize(v); localStorage.setItem("fontSize", v); document.body.style.zoom = `${parseInt(v) / 14 * 100}%`; }} style={{ maxWidth: 200 }}>
                <option value="12">12px - 小</option>
                <option value="14">14px - 默认</option>
                <option value="16">16px - 中</option>
                <option value="18">18px - 大</option>
              </select>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
