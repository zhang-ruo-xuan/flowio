import { useState, useEffect, useCallback, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import {
  Moon, Sun, Zap, Palette, ArrowLeft, Eye, EyeOff,
  Star, Trash2, Check, Plus, Shield, Circle, User, LogOut, RefreshCw, Keyboard,
} from "lucide-react";
import { useAuthStore } from "../store/authStore";

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
const PB_URL = "https://flowio.cn";

const kbdStyle: React.CSSProperties = {
  display: "inline-block",
  padding: "2px 8px",
  borderRadius: 5,
  border: "1px solid var(--border)",
  borderBottomWidth: 2,
  background: "var(--bg)",
  color: "var(--text)",
  fontSize: 12.5,
  fontWeight: 600,
  fontFamily: "Consolas, monospace",
  minWidth: 110,
  textAlign: "center",
};

export default function SettingsPage({ darkMode, onToggleDark }: SettingsPageProps) {
  const navigate = useNavigate();
  const [activeTab, setActiveTab] = useState<"ai" | "appearance" | "redaction" | "recording" | "hotkeys" | "profile">("ai");
  const { siteUser, siteToken, clearAuth, setAuth } = useAuthStore();

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

    const modelList = customModels.split(/[,，]/).map((s) => s.trim()).filter(Boolean);

    // Validate API key before saving
    try {
      await invoke("validate_custom_api", {
        baseUrl: customBaseUrl.trim(),
        apiKey: customApiKey.trim(),
        model: modelList[0] || undefined,
      });
    } catch (e: unknown) {
      setCustomSaveResult(`API 验证失败: ${String(e)}`);
      setCustomSaving(false);
      return;
    }

    const id = "custom_" + Date.now();
    const newProv: ProviderDef = { id, name: customName, base_url: customBaseUrl, models: modelList, custom: true };

    try {
      await invoke("add_custom_provider", {
        provider: { id, name: customName, base_url: customBaseUrl, models: modelList },
        apiKey: customApiKey,
      });
      setProviders((prev) => [...prev, newProv]);
      setProviderConfigs((prev) => ({ ...prev, [id]: { model: modelList[0] || "", apiKey: customApiKey, configured: !!customApiKey } }));
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

  /* ---- Font Size ---- */
  const [fontSize, setFontSize] = useState(() => localStorage.getItem("fontSize") || "14");
  useEffect(() => {
    document.body.style.zoom = `${parseInt(fontSize) / 14 * 100}%`;
  }, [fontSize]);

  /* ---- Redaction Settings ---- */
  // 导出前自动脱敏默认关闭：用户日常导出通常就是给团队/客户看的原始录屏；
  // 真要脱敏用编辑器里的「文本脱敏」「扫描脱敏」按需手动触发。
  const [redactCfg, setRedactCfg] = useState<{ enabled: string[]; custom_patterns: string[]; mode: "blur" | "block"; redact_before_export: boolean }>({
    enabled: [],
    custom_patterns: [],
    mode: "blur",
    redact_before_export: false,
  });
  const [redactSaved, setRedactSaved] = useState<string | null>(null);

  /* ---- Recording Settings ---- */
  const [recordSelf, setRecordSelf] = useState(() => localStorage.getItem("flowio_record_self") === "1");

  const handleRecordSelfToggle = (value: boolean) => {
    setRecordSelf(value);
    localStorage.setItem("flowio_record_self", value ? "1" : "0");
  };

  /* ---- Profile State ---- */
  const [profileRefreshing, setProfileRefreshing] = useState(false);
  const [profileSyncMsg, setProfileSyncMsg] = useState("");
  const [profileAvatarError, setProfileAvatarError] = useState(false);

  const profileAvatarUrl = siteUser?.avatar
    ? `${PB_URL}/api/files/users/${siteUser.id}/${siteUser.avatar}`
    : null;

  const profileName = siteUser?.display_name || siteUser?.email?.split("@")[0] || siteUser?.id?.slice(-8) || "?";

  const doProfileRefresh = async () => {
    setProfileRefreshing(true);
    setProfileSyncMsg("");
    try {
      const res = await fetch(`${PB_URL}/api/collections/users/auth-refresh`, {
        method: "POST",
        headers: { Authorization: siteToken || "" },
      });
      if (res.ok) {
        const data = await res.json();
        if (data.token && data.record?.id) {
          setAuth(data.token, {
            id: data.record.id,
            email: data.record.email || "",
            display_name: data.record.display_name || "",
            bio: data.record.bio || "",
            avatar: data.record.avatar || "",
          });
          setProfileAvatarError(false);
          setProfileSyncMsg("已同步");
          setTimeout(() => setProfileSyncMsg(""), 2000);
        }
      } else {
        setProfileSyncMsg("同步失败");
        setTimeout(() => setProfileSyncMsg(""), 2000);
      }
    } catch {
      setProfileSyncMsg("同步失败");
      setTimeout(() => setProfileSyncMsg(""), 2000);
    }
    setProfileRefreshing(false);
  };

  const doProfileLogout = () => {
    clearAuth();
    navigate("/");
  };

  useEffect(() => {
    (async () => {
      try {
        const cfg = await invoke<{ enabled?: string[]; enabled_kinds?: string[]; custom_patterns?: string[]; mode?: "blur" | "block"; redact_before_export?: boolean }>("get_redaction_config");
        setRedactCfg({
          enabled: cfg.enabled ?? cfg.enabled_kinds ?? [],
          custom_patterns: cfg.custom_patterns ?? [],
          mode: cfg.mode === "block" ? "block" : "blur",
          redact_before_export: cfg.redact_before_export ?? false,
        });
      } catch { /* fallback to defaults */ }
    })();
  }, []);

  const REDACT_KIND_OPTIONS: Array<{ key: string; label: string }> = [
    { key: "id_card", label: "身份证号" },
    { key: "phone", label: "手机号" },
    { key: "bank_card", label: "银行卡号" },
    { key: "email", label: "邮箱" },
    { key: "api_key", label: "API Key / Token" },
    { key: "credit_code", label: "统一社会信用代码" },
    { key: "qq_wechat", label: "QQ号 / 微信号" },
    { key: "password_field", label: "密码 / 口令" },
    { key: "ip_mac", label: "内网IP / MAC" },
    { key: "url_param", label: "URL 敏感参数" },
    { key: "custom", label: "自定义规则" },
  ];

  const handleRedactKindToggle = async (key: string) => {
    const prev = redactCfg;
    const next = {
      ...prev,
      enabled: redactCfg.enabled.includes(key)
        ? redactCfg.enabled.filter((k) => k !== key)
        : [...redactCfg.enabled, key],
    };
    setRedactCfg(next);
    setRedactSaved(null);
    try {
      await invoke("set_redaction_config", { config: next });
      setRedactSaved("已保存");
    } catch (e) {
      setRedactSaved(`保存失败: ${String(e)}`);
      setRedactCfg(prev);
    }
  };

  const handleRedactModeChange = async (mode: "blur" | "block") => {
    const prev = redactCfg;
    const next = { ...redactCfg, mode };
    setRedactCfg(next);
    setRedactSaved(null);
    try {
      await invoke("set_redaction_config", { config: next });
      setRedactSaved("已保存");
    } catch (e) {
      setRedactSaved(`保存失败: ${String(e)}`);
      setRedactCfg(prev);
    }
  };

  const handleRedactExportToggle = async (value: boolean) => {
    const prev = redactCfg;
    const next = { ...redactCfg, redact_before_export: value };
    setRedactCfg(next);
    setRedactSaved(null);
    try {
      await invoke("set_redaction_config", { config: next });
      setRedactSaved("已保存");
    } catch (e) {
      setRedactSaved(`保存失败: ${String(e)}`);
      setRedactCfg(prev);
    }
  };

  const handleRedactCustomPattern = async (index: number, value: string) => {
    const prev = redactCfg;
    const patterns = [...redactCfg.custom_patterns];
    patterns[index] = value;
    const next = { ...redactCfg, custom_patterns: patterns };
    setRedactCfg(next);
    setRedactSaved(null);
    try {
      await invoke("set_redaction_config", { config: next });
      setRedactSaved("已保存");
    } catch (e) {
      setRedactSaved(`保存失败: ${String(e)}`);
      setRedactCfg(prev);
    }
  };

  const handleRedactAddPattern = async () => {
    const prev = redactCfg;
    const next = { ...redactCfg, custom_patterns: [...redactCfg.custom_patterns, ""] };
    setRedactCfg(next);
    setRedactSaved(null);
    try {
      await invoke("set_redaction_config", { config: next });
      setRedactSaved("已保存");
    } catch (e) {
      setRedactSaved(`保存失败: ${String(e)}`);
      setRedactCfg(prev);
    }
  };

  const handleRedactRemovePattern = async (index: number) => {
    const prev = redactCfg;
    const next = { ...redactCfg, custom_patterns: redactCfg.custom_patterns.filter((_, i) => i !== index) };
    setRedactCfg(next);
    setRedactSaved(null);
    try {
      await invoke("set_redaction_config", { config: next });
      setRedactSaved("已保存");
    } catch (e) {
      setRedactSaved(`保存失败: ${String(e)}`);
      setRedactCfg(prev);
    }
  };

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
        <button className={`tab ${activeTab === "appearance" ? "active" : ""}`} onClick={() => setActiveTab("appearance")}>
          <Palette size={14} style={{ marginRight: 4, verticalAlign: "middle" }} />外观
        </button>
        <button className={`tab ${activeTab === "redaction" ? "active" : ""}`} onClick={() => setActiveTab("redaction")}>
          <Shield size={14} style={{ marginRight: 4, verticalAlign: "middle" }} />脱敏
        </button>
        <button className={`tab ${activeTab === "recording" ? "active" : ""}`} onClick={() => setActiveTab("recording")}>
          <Circle size={14} style={{ marginRight: 4, verticalAlign: "middle" }} />录制
        </button>
        <button className={`tab ${activeTab === "hotkeys" ? "active" : ""}`} onClick={() => setActiveTab("hotkeys")}>
          <Keyboard size={14} style={{ marginRight: 4, verticalAlign: "middle" }} />快捷键
        </button>
        <button className={`tab ${activeTab === "profile" ? "active" : ""}`} onClick={() => setActiveTab("profile")}>
          <User size={14} style={{ marginRight: 4, verticalAlign: "middle" }} />个人中心
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
                        <label>模型</label>
                        <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                          <input value={customModels} onChange={(e) => setCustomModels(e.target.value)} placeholder="例如: glm-4v, qwen-vl-plus（多个用逗号分隔）" style={{ flex: 1 }} />
                        </div>
                        <p className="hint">支持逗号分隔填写多个模型名</p>
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

                      {selectedProv?.custom ? (
                      <div className="form-group" style={{ marginBottom: 14 }}>
                        <label>模型</label>
                        <input value={editModel} onChange={(e) => setEditModel(e.target.value)} placeholder="输入模型名，多个用逗号分隔" />
                        <p className="hint">自定义服务商可手动输入模型名</p>
                      </div>
                      ) : (
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

        {/* ── 脱敏 ── */}
        {activeTab === "redaction" && (
          <div>
            <div className="form-group">
              <label>导出前自动脱敏</label>
              <div className="toggle-switch" onClick={() => handleRedactExportToggle(!redactCfg.redact_before_export)}>
                <div className={`toggle-switch-track${redactCfg.redact_before_export ? " active" : ""}`}>
                  <div className="toggle-switch-thumb" />
                </div>
                <span className="toggle-switch-label">
                  {redactCfg.redact_before_export ? "开启" : "关闭"}
                </span>
              </div>
              <p className="hint">开启后，导出 HTML / Markdown / PDF / Word / 长图时自动扫描并脱敏截图中的敏感内容（文本替换为 [已隐藏]），原图与本地数据保持不动</p>
            </div>

            <div className="form-group">
              <label>检测类别</label>
              <p className="hint">取消勾选后，隐私脱敏与分享前检查将跳过对应类别；点击「隐私脱敏」时按此配置执行</p>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "6px 16px", marginTop: 8 }}>
                {REDACT_KIND_OPTIONS.map((opt) => {
                  const enabled = redactCfg.enabled.includes(opt.key);
                  return (
                    <label key={opt.key} style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer", fontSize: 14 }}>
                      <input
                        type="checkbox"
                        checked={enabled}
                        onChange={() => handleRedactKindToggle(opt.key)}
                        style={{ accentColor: "var(--primary, #3366CC)", width: 16, height: 16 }}
                      />
                      {opt.label}
                    </label>
                  );
                })}
              </div>
            </div>

            <div className="form-group">
              <label>截图覆盖方式</label>
              <p className="hint">「扫描截图敏感区域」时对命中的文字区域采用的遮盖方式；手动框选脱敏始终使用模糊</p>
              <div style={{ display: "flex", gap: 20, marginTop: 8 }}>
                <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer", fontSize: 14 }}>
                  <input
                    type="radio"
                    name="redact-mode"
                    checked={redactCfg.mode === "blur"}
                    onChange={() => handleRedactModeChange("blur")}
                    style={{ accentColor: "var(--primary, #3366CC)", width: 16, height: 16 }}
                  />
                  高斯模糊
                </label>
                <label style={{ display: "flex", alignItems: "center", gap: 8, cursor: "pointer", fontSize: 14 }}>
                  <input
                    type="radio"
                    name="redact-mode"
                    checked={redactCfg.mode === "block"}
                    onChange={() => handleRedactModeChange("block")}
                    style={{ accentColor: "var(--primary, #3366CC)", width: 16, height: 16 }}
                  />
                  实心色块覆盖
                </label>
              </div>
            </div>

            <div className="form-group">
              <label>自定义正则规则</label>
              <p className="hint">按行填写正则表达式（JS/ECMAScript 语法，Rust 侧同样生效），命中内容将替换为 [已隐藏]</p>
              {redactCfg.custom_patterns.length === 0 && (
                <p style={{ fontSize: 13, color: "var(--text-tertiary)", margin: "6px 0" }}>暂无自定义规则</p>
              )}
              {redactCfg.custom_patterns.map((pat, i) => (
                <div key={i} style={{ display: "flex", gap: 8, marginBottom: 6, alignItems: "center" }}>
                  <input
                    type="text"
                    value={pat}
                    placeholder="例如：\b(?:员工编号|工号)[:：]\s*\d{4,}\b"
                    onChange={(e) => handleRedactCustomPattern(i, e.target.value)}
                    style={{ flex: 1 }}
                  />
                  <button
                    onClick={() => handleRedactRemovePattern(i)}
                    style={{ fontSize: 12, padding: "4px 10px", border: "1px solid var(--border)", borderRadius: 4, background: "transparent", cursor: "pointer", color: "var(--red)" }}
                  >
                    删除
                  </button>
                </div>
              ))}
              <button
                onClick={handleRedactAddPattern}
                style={{ fontSize: 13, padding: "6px 14px", border: "1px dashed var(--border)", borderRadius: 4, background: "transparent", cursor: "pointer", color: "var(--text-secondary)", marginTop: 4 }}
              >
                + 添加正则
              </button>
            </div>

            {redactSaved && <span style={{ fontSize: 13, color: redactSaved === "已保存" ? "var(--green)" : "var(--red)" }}>{redactSaved}</span>}

            <div style={{ marginTop: 20, padding: "12px 14px", borderRadius: 8, border: "1px solid var(--border)", background: "var(--bg-secondary)", fontSize: 13, color: "var(--text-secondary)", lineHeight: 1.7 }}>
              <b>隐私与合规说明</b><br />
              · 所有检测与脱敏均在<b>本地</b>完成，截图与文本不会上传到任何服务器；<br />
              · 导出前自动脱敏会生成脱敏副本，原始截图与数据库数据保持不变。
            </div>
          </div>
        )}

        {/* ── 快捷键 ── */}
        {activeTab === "hotkeys" && (
          <div>
            <div style={{ padding: "16px 18px", borderRadius: 10, border: "1px solid var(--border)", background: "var(--bg-secondary)" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 12 }}>
                <Keyboard size={15} style={{ color: "var(--blue)" }} />
                <span style={{ fontSize: 14, fontWeight: 600, color: "var(--text)" }}>全局快捷键</span>
                <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>（所有界面下均可使用）</span>
              </div>
              <div style={{ display: "flex", flexDirection: "column", gap: 10, fontSize: 13.5, color: "var(--text-secondary)" }}>
                <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
                  <kbd style={kbdStyle}>Alt + Shift + S</kbd>
                  <span>开始录制 / 恢复录制</span>
                </div>
                <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
                  <kbd style={kbdStyle}>Alt + Shift + P</kbd>
                  <span>暂停 / 继续录制</span>
                </div>
                <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
                  <kbd style={kbdStyle}>Alt + Shift + E</kbd>
                  <span>停止录制并进入编辑器</span>
                </div>
              </div>
            </div>
          </div>
        )}

        {/* ── 录制 ── */}
        {activeTab === "recording" && (
          <div>
            <div className="form-group">
              <label>录制录步自身操作</label>
              <div className="toggle-switch" onClick={() => handleRecordSelfToggle(!recordSelf)}>
                <div className={`toggle-switch-track${recordSelf ? " active" : ""}`}>
                  <div className="toggle-switch-thumb" />
                </div>
                <span className="toggle-switch-label">
                  {recordSelf ? "开启" : "关闭"}
                </span>
              </div>
              <p className="hint">开启后，录制过程中也会捕获录步窗口自身的操作，用于制作「录步使用手册」等教程。全局快捷键：Alt+Shift+S 开始/恢复录制，Alt+Shift+P 暂停/继续，Alt+Shift+E 停止录制。录制时窗口会自动最小化，按钮点击也会被如实录入。</p>
            </div>
          </div>
        )}

        {/* ── 个人中心 ── */}
        {activeTab === "profile" && (
          <div>
            {siteUser ? (
              <div style={{ border: "1px solid var(--border)", borderRadius: 12, padding: "28px 32px", background: "var(--bg)", maxWidth: 460 }}>
                <div style={{ display: "flex", alignItems: "center", gap: 16, marginBottom: 20 }}>
                  {profileAvatarUrl && !profileAvatarError ? (
                    <img src={profileAvatarUrl} alt="avatar" onError={() => setProfileAvatarError(true)} style={{ width: 56, height: 56, borderRadius: "50%", objectFit: "cover", border: "1px solid var(--border)" }} />
                  ) : (
                    <div style={{ width: 56, height: 56, borderRadius: "50%", background: "var(--green)", color: "#fff", display: "flex", alignItems: "center", justifyContent: "center", fontSize: 22, fontWeight: 600 }}>
                      {profileName.charAt(0).toUpperCase()}
                    </div>
                  )}
                  <div>
                    <div style={{ fontSize: 18, fontWeight: 600, color: "var(--text)", lineHeight: 1.3 }}>{profileName}</div>
                    {siteUser.bio && <div style={{ fontSize: 13, color: "var(--text-secondary)", marginTop: 2 }}>{siteUser.bio}</div>}
                    <div style={{ fontSize: 12, color: "var(--text-tertiary)", marginTop: 2 }}>{siteUser.email}</div>
                  </div>
                </div>
                <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                  <button className="btn btn-primary" onClick={doProfileRefresh} disabled={profileRefreshing} style={{ background: "var(--green)", display: "inline-flex", alignItems: "center", justifyContent: "center", gap: 6 }}>
                    <RefreshCw size={15} style={{ animation: profileRefreshing ? "spin 1s linear infinite" : "none" }} />{profileRefreshing ? "同步中..." : "刷新用户信息"}
                  </button>
                  {profileSyncMsg && <span style={{ fontSize: 13, color: profileSyncMsg === "已同步" ? "var(--green)" : "var(--red)" }}>{profileSyncMsg}</span>}
                  <button className="btn" onClick={() => navigate("/profile")} style={{ display: "inline-flex", alignItems: "center", justifyContent: "center", gap: 6 }}>
                    <User size={15} />进入个人中心
                  </button>
                  <button onClick={doProfileLogout} style={{ display: "inline-flex", alignItems: "center", justifyContent: "center", gap: 6, background: "transparent", border: "1px solid var(--red)", borderRadius: 6, padding: "9px 14px", cursor: "pointer", color: "var(--red)", fontSize: 14 }}>
                    <LogOut size={15} />退出登录
                  </button>
                </div>
              </div>
            ) : (
              <div style={{ border: "1px dashed var(--border)", borderRadius: 12, padding: "28px 32px", maxWidth: 460, textAlign: "center" }}>
                <div style={{ fontSize: 15, color: "var(--text-secondary)", marginBottom: 14 }}>未登录网站账号</div>
                <button className="btn btn-primary" onClick={() => invoke("open_external_url", { url: "https://flowio.cn/author/login?redirect=flowio://auth" })} style={{ background: "var(--green)" }}>
                  登录网站账号
                </button>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
