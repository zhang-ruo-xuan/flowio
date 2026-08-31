# 下载页 SmartScreen 引导文案

> 用途：录步 Flowio 桌面端目前未购买代码签名证书，Windows 会从互联网下载的 exe 弹出
> SmartScreen 拦截。把下面内容贴到官网下载页，引导用户安全放行。
> （上了 Microsoft Store 或买了签名证书后，此提示可移除。）

---

## 一、纯文案版（直接粘贴到下载页说明区）

### 关于 Windows 打开时的安全提示

在 Windows 上首次运行「录步 Flowio」安装包时，系统可能会弹出 **Windows Defender SmartScreen**
的安全提醒，提示"Windows 已保护你的电脑"或"无法识别的应用"。

**这是正常现象，不代表软件有问题。**

原因是：录步 Flowio 桌面端目前尚未购买商业代码签名证书，Windows 对"从互联网下载、
未经证书签名"的应用会统一弹出此提醒。我们计划在正式版阶段接入代码签名（或上架
Microsoft Store），届时该提示会自动消失。

**如何继续安装（约 5 秒）：**

1. 在弹出的窗口中，点击左下角的 **「更多信息」**（蓝色文字链接）
2. 窗口会展开，出现一个 **「仍要运行」** 按钮，点击它
3. 安装程序正常启动，按提示完成即可

若系统提示"已阻止此发布者"，同样点击「更多信息 → 仍要运行」即可。

**你完全可以放心：** 录步 Flowio 是**本地优先**的录屏/教程工具，所有录制数据默认只存在
你自己的浏览器与电脑里，不会在未经你许可的情况下上传。我们正在推进开源与代码签名，
欢迎在官网或社区核对我们的代码与隐私承诺。

---

## 二、HTML 卡片版（可直接嵌入下载页，样式已对齐品牌深灰 #24292e）

```html
<div class="smartscreen-notice">
  <div class="sn-icon">🛡️</div>
  <div class="sn-body">
    <h3>打开安装包时看到安全提示？</h3>
    <p>
      这是 Windows 的 <strong>SmartScreen</strong> 对"未签名应用"的常规提醒，<strong>不代表软件有问题</strong>。
      录步 Flowio 目前尚未购买代码签名证书，正式版将接入签名 / 上架 Microsoft Store 后自动消失。
    </p>
    <ol>
      <li>在弹窗中点左下角 <strong>「更多信息」</strong></li>
      <li>点击出现的 <strong>「仍要运行」</strong> 按钮</li>
      <li>安装程序正常启动，按提示完成即可</li>
    </ol>
    <p class="sn-foot">
      本地优先 · 数据默认只存你自己的设备 · 我们正在推进开源与代码签名
    </p>
  </div>
</div>

<style>
.smartscreen-notice {
  display: flex; gap: 16px; align-items: flex-start;
  background: #f6f7f9; border: 1px solid #e3e5e8; border-left: 4px solid #24292e;
  border-radius: 10px; padding: 18px 20px; margin: 24px 0;
  font-family: -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif; color: #24292e;
}
.smartscreen-notice .sn-icon { font-size: 28px; line-height: 1; }
.smartscreen-notice h3 { margin: 0 0 8px; font-size: 16px; }
.smartscreen-notice p { margin: 0 0 10px; font-size: 14px; line-height: 1.7; color: #444; }
.smartscreen-notice ol { margin: 0 0 10px; padding-left: 20px; font-size: 14px; line-height: 1.9; }
.smartscreen-notice .sn-foot { margin: 0; font-size: 12px; color: #888; }
</style>
```

---

## 三、上架 Microsoft Store 后的替代说明（届时替换上面内容）

> 已上架 Microsoft Store：请从 Microsoft Store 搜索"录步 Flowio"安装，无需任何安全提示，
> 一键安装、自动更新。
