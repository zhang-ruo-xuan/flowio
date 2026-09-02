import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { ArrowLeft, Settings, LogOut, RefreshCw, User } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { useAuthStore } from "../store/authStore";

const PB_URL = "https://flowio.cn";

export default function ProfilePage() {
  const navigate = useNavigate();
  const { siteUser, siteToken, clearAuth, setAuth } = useAuthStore();
  const [refreshing, setRefreshing] = useState(false);
  const [syncMsg, setSyncMsg] = useState("");
  const [avatarError, setAvatarError] = useState(false);

  const avatarUrl = siteUser?.avatar ? `${PB_URL}/api/files/users/${siteUser.id}/${siteUser.avatar}` : null;
  const name = siteUser?.display_name || siteUser?.id?.slice(-8) || "未登录";

  const doRefresh = async () => {
    setRefreshing(true);
    setSyncMsg("");
    try {
      const res = await fetch(`${PB_URL}/api/collections/users/auth-refresh`, {
        method: "POST",
        headers: { Authorization: siteToken || "" },
      });
      if (res.ok) {
        const data = await res.json();
        if (data.token && data.record?.id) {
          const prev = siteUser;
          setAuth(data.token, {
            id: data.record.id,
            email: data.record.email || prev?.email || "",
            display_name: data.record.display_name || prev?.display_name || "",
            bio: data.record.bio || prev?.bio || "",
            avatar: data.record.avatar || prev?.avatar || "",
          });
          setSyncMsg("已同步");
          setTimeout(() => setSyncMsg(""), 2000);
        }
      } else {
        setSyncMsg("同步失败");
        setTimeout(() => setSyncMsg(""), 2000);
      }
    } catch {
      setSyncMsg("同步失败");
      setTimeout(() => setSyncMsg(""), 2000);
    }
    setRefreshing(false);
  };

  const doLogout = () => {
    clearAuth();
    // 清除本地 DB 隔离标记，列表回到未登录（__local__）视图
    invoke("set_current_user", { userId: "" }).catch((e) =>
      console.warn("[flowio] 清除 current_user 失败:", e)
    );
    navigate("/");
  };

  const login = () => invoke("open_external_url", { url: "https://flowio.cn/author/login?redirect=flowio://auth" });

  return (
    <div className="page">
      <div style={{ display: "flex", alignItems: "center", padding: "8px 0" }}>
        <button
          onClick={() => navigate("/")}
          style={{
            display: "inline-flex", alignItems: "center", gap: 4,
            background: "transparent", border: "none", cursor: "pointer",
            padding: "6px 12px", borderRadius: 6, fontSize: 14, color: "var(--blue)",
          }}
          onMouseEnter={(e) => (e.currentTarget.style.background = "rgba(0,0,0,0.06)")}
          onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
        >
          <ArrowLeft size={16} />返回
        </button>
      </div>

      <div className="page-header"><h1>个人中心</h1></div>

      <div className="content-area" style={{ marginTop: 16 }}>
        {!siteUser && (
          <div
            style={{
              border: "1px solid #fde68a",
              background: "#fffbeb",
              borderRadius: 8,
              padding: "10px 14px",
              marginBottom: 16,
              fontSize: 13,
              color: "#92400e",
              lineHeight: 1.6,
            }}
          >
            登录网站账号后会自动同步；如果一直没反应，点下方按钮手动同步。
          </div>
        )}
        {siteUser ? (
          <div style={{ maxWidth: 480 }}>
            <div style={{ fontSize: 13, color: "var(--text-muted)", marginBottom: 10 }}>当前登录账号</div>

            {/* 当前账号卡片 */}
            <div
              style={{
                display: "flex", alignItems: "center", gap: 12,
                border: "1px solid var(--blue)",
                borderRadius: 12, padding: "12px 14px", background: "var(--bg)",
              }}
            >
              <div
                style={{
                  width: 40, height: 40, borderRadius: "50%", flexShrink: 0,
                  background: avatarUrl && !avatarError ? "transparent" : "#4ade80",
                  display: "flex", alignItems: "center", justifyContent: "center",
                  fontSize: 16, fontWeight: 700, color: avatarUrl && !avatarError ? "inherit" : "#000",
                  overflow: "hidden",
                }}
              >
                {avatarUrl && !avatarError ? (
                  <img src={avatarUrl} alt={name} style={{ width: "100%", height: "100%", objectFit: "cover" }} onError={() => setAvatarError(true)} />
                ) : (
                  <User size={20} strokeWidth={2.5} />
                )}
              </div>

              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 14, fontWeight: 600 }}>{name}</div>
                {siteUser.bio && (
                  <div style={{ fontSize: 12, color: "var(--text-muted)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {siteUser.bio}
                  </div>
                )}
              </div>

              <button
                onClick={doLogout}
                style={{
                  flexShrink: 0, display: "inline-flex", alignItems: "center", gap: 4,
                  padding: "6px 10px", borderRadius: 6, fontSize: 12,
                  border: "1px solid #fecaca", background: "transparent",
                  cursor: "pointer", color: "var(--red)",
                }}
              >
                <LogOut size={13} />退出
              </button>
            </div>

            {/* 账号操作 */}
            <div style={{ marginTop: 16, display: "flex", flexDirection: "column", gap: 8 }}>
              <button
                onClick={doRefresh}
                disabled={refreshing}
                style={{
                  display: "inline-flex", alignItems: "center", justifyContent: "center", gap: 6,
                  padding: "10px 16px", borderRadius: 8, fontSize: 14, fontWeight: 500,
                  border: "1px solid var(--border)", background: "transparent",
                  cursor: "pointer", color: "var(--text)",
                }}
              >
                <RefreshCw size={14} style={{ animation: refreshing ? "spin 1s linear infinite" : "none" }} />
                {refreshing ? "同步中..." : "刷新用户信息"}
              </button>
              {syncMsg && (
                <span style={{ fontSize: 13, color: syncMsg === "已同步" ? "var(--green)" : "var(--red)" }}>
                  {syncMsg}
                </span>
              )}
              <button
                onClick={() => navigate("/settings")}
                style={{
                  display: "flex", alignItems: "center", gap: 10,
                  padding: "12px 16px", borderRadius: 8, fontSize: 14,
                  border: "1px solid var(--border)", background: "transparent",
                  cursor: "pointer", color: "var(--text)", width: "100%", textAlign: "left",
                }}
                onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-secondary)")}
                onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
              >
                <Settings size={16} style={{ color: "var(--text-muted)" }} />
                设置
              </button>
            </div>
          </div>
        ) : (
          <div
            style={{
              border: "1px solid var(--border)",
              borderRadius: 12,
              padding: "48px 36px",
              textAlign: "center",
              maxWidth: 480,
            }}
          >
            <div
              style={{
                width: 56, height: 56, borderRadius: "50%",
                background: "var(--bg-secondary)",
                display: "flex", alignItems: "center", justifyContent: "center",
                margin: "0 auto 16px",
              }}
            >
              <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" style={{ color: "var(--text-muted)" }}>
                <circle cx="12" cy="8" r="4" /><path d="M20 21a8 8 0 1 0-16 0" />
              </svg>
            </div>
            <div style={{ fontSize: 16, fontWeight: 600, marginBottom: 6 }}>未登录</div>
            <div style={{ fontSize: 14, color: "var(--text-muted)", marginBottom: 20 }}>
              登录后可同步网站账号信息
            </div>
            <button
              onClick={login}
              style={{
                padding: "8px 24px", borderRadius: 6, fontSize: 14, fontWeight: 500,
                border: "none", background: "var(--blue)", color: "#fff",
                cursor: "pointer",
              }}
            >
              登录网站账号
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
