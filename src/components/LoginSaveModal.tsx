import { createPortal } from "react-dom";
import { CloudUpload, Check, X, ArrowRight } from "lucide-react";

/**
 * 「保存到云端？」登录引导弹窗（Scribe 风格）
 * - 仅在未登录时展示；录制结束进入编辑界面后自动弹出
 * - 点击「登录并保存」→ 打开网站登录页（登录用于上传保存数据）
 * - 点击「暂不需要」/ × → 关闭，本次会话内不再重复弹出
 */
interface LoginSaveModalProps {
  open: boolean;
  onClose: () => void;
  onLogin: () => void;
}

export default function LoginSaveModal({ open, onClose, onLogin }: LoginSaveModalProps) {
  if (!open) return null;

  const s: Record<string, React.CSSProperties> = {
    overlay: {
      position: "fixed",
      inset: 0,
      zIndex: 11000,
      background: "rgba(0, 0, 0, 0.4)",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
    },
    dialog: {
      width: 400,
      maxWidth: "90vw",
      background: "#fff",
      borderRadius: 16,
      padding: "28px 28px 20px",
      boxShadow: "0 16px 48px rgba(0,0,0,0.2)",
      position: "relative",
      textAlign: "center",
    },
    close: {
      position: "absolute",
      top: 12,
      right: 12,
      border: "none",
      background: "transparent",
      cursor: "pointer",
      color: "#9ca3af",
      padding: 4,
      borderRadius: 6,
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
    },
    iconWrap: {
      width: 64,
      height: 64,
      borderRadius: "50%",
      background: "#ff6b35",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      margin: "0 auto 16px",
    },
    title: {
      fontSize: 20,
      fontWeight: 700,
      color: "#111827",
      marginBottom: 10,
    },
    desc: {
      fontSize: 13.5,
      lineHeight: 1.6,
      color: "#4b5563",
      margin: "0 0 18px",
    },
    features: {
      textAlign: "left",
      margin: "0 0 22px",
      padding: 0,
      listStyle: "none",
    },
    featureItem: {
      display: "flex",
      alignItems: "center",
      gap: 8,
      fontSize: 13.5,
      color: "#374151",
      padding: "5px 0",
    },
    check: {
      flexShrink: 0,
      width: 20,
      height: 20,
      borderRadius: "50%",
      background: "#ff6b35",
      color: "#fff",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
    },
    primaryBtn: {
      width: "100%",
      border: "none",
      background: "#ff6b35",
      color: "#fff",
      fontSize: 15,
      fontWeight: 600,
      padding: "12px 0",
      borderRadius: 10,
      cursor: "pointer",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      gap: 6,
      transition: "background 0.15s",
    },
    secondaryBtn: {
      display: "block",
      margin: "12px auto 0",
      border: "none",
      background: "transparent",
      color: "#6b7280",
      fontSize: 13,
      cursor: "pointer",
      padding: "4px 8px",
    },
    footnote: {
      margin: "16px 0 0",
      fontSize: 11.5,
      color: "#9ca3af",
      lineHeight: 1.5,
    },
  };

  return createPortal(
    <div style={s.overlay} onClick={onClose}>
      <div style={s.dialog} onClick={(e) => e.stopPropagation()}>
        <button style={s.close} onClick={onClose} data-tip="关闭，只留在这台电脑上">
          <X size={18} />
        </button>

        <div style={s.iconWrap}>
          <CloudUpload size={32} color="#fff" />
        </div>

        <div style={s.title}>保存到云端？</div>

        <p style={s.desc}>
          当前录制内容只保存在这台电脑上，登录后即可上传保存数据，
          换设备或清理本地数据也不会丢。
        </p>

        <ul style={s.features}>
          <li style={s.featureItem}>
            <span style={s.check}><Check size={13} strokeWidth={3} /></span>
            发布到网站，生成可分享的链接
          </li>
          <li style={s.featureItem}>
            <span style={s.check}><Check size={13} strokeWidth={3} /></span>
            多设备随时打开、继续编辑
          </li>
          <li style={s.featureItem}>
            <span style={s.check}><Check size={13} strokeWidth={3} /></span>
            自动备份，不再怕本地数据丢失
          </li>
        </ul>

        <button style={s.primaryBtn} onClick={onLogin} data-tip="去 flowio.cn 登录，之后把这份录制同步到云端">
          <ArrowRight size={16} />
          登录并保存
        </button>

        <button style={s.secondaryBtn} onClick={onClose} data-tip="不上传，继续留在本地（之后想传随时可以）">
          暂不需要
        </button>

        <p style={s.footnote}>
          不登录也能继续编辑、导出和保存，随时可以在右上角登录。
        </p>
      </div>
    </div>,
    document.body
  );
}
