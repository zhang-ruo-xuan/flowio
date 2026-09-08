# 录步 Flowio 安装包返回码说明

适用于录步 Flowio 0.2.0 及以上版本 EXE 安装包（基于 NSIS 3.x 构建）。

## 返回码对照

| 返回码 | 含义 |
|--------|------|
| 0      | 安装成功 |
| 1      | 安装失败（权限不足 / 磁盘空间不足 / 被安全软件拦截） |
| 2      | 用户主动取消安装 |
| 其他非 0 | 安装异常终止；请记录返回码并联系 support@flowio.cn |

## 常见失败排查

- **返回 1**：右键安装包 → 以管理员身份运行；或临时关闭杀毒软件后重试。
- **返回 2**：安装向导中点选了"取消"。
- 安装日志路径：`%LOCALAPPDATA%\flowio\`

## 静默安装

支持 NSIS 标准静默参数：

```
Flowio-Setup-<version>.exe /S
```

## 获取帮助

- 提交问题：https://github.com/zhang-ruo-xuan/flowio/issues
- 官网：https://flowio.cn
