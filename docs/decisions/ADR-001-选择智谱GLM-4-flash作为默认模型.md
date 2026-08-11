---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_282d408b882c11f1a68c525400826444
    ReservedCode1: pk4eaawTuaQApjphiJKxme4lgv10WvnGmqTxg7YOD/BALnmBrd2AjZUL9IITQFgxyBlpKyUyPYrc6BasqiTagjMXUGmCq3qMI1zugmv3ZcDkatKWSsZniqgBUvEFoHHTg/LIx/j+4u6oeRsCt/DGyWbo631/Wj7A0h31xW9c3Jdu4j4xoVhnrvApui0=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_282d408b882c11f1a68c525400826444
    ReservedCode2: pk4eaawTuaQApjphiJKxme4lgv10WvnGmqTxg7YOD/BALnmBrd2AjZUL9IITQFgxyBlpKyUyPYrc6BasqiTagjMXUGmCq3qMI1zugmv3ZcDkatKWSsZniqgBUvEFoHHTg/LIx/j+4u6oeRsCt/DGyWbo631/Wj7A0h31xW9c3Jdu4j4xoVhnrvApui0=
---

# ADR-001：选择智谱 GLM-4-flash 作为默认模型

> 状态：已采纳
> 日期：2026-07-25
> 决策者：录步 (Flowio) 项目

## 背景

录步的核心功能依赖大模型生成中文步骤说明。在选型阶段面临三个关键问题：

1. **成本控制**：MVP 阶段用户规模未知，AI 调用成本必须在可控范围内
2. **合规要求**：面向中国市场的国产化工具，必须优先考虑数据不出境
3. **切换成本**：用户可能有不同模型偏好，架构需支持灵活切换

成本测算基准：

| 版本 | 月调用次数 | 单次成本 | 月 AI 总成本 | 订阅价 | 毛利 |
|------|-----------|----------|-------------|--------|------|
| 免费版 | 20 次 | ¥0.02 | ¥0.4 | ¥0 | 引流 |
| 个人版 | 500 次 | ¥0.02 | ¥10 | ¥29 | 66% |
| 团队版 | 无限 | ¥0.02 | — | ¥79/seat | 需按实际核算 |

候选模型对比：

| 模型 | 单价 (¥/万Token) | 单次约 (¥) | 合规 | 备选 |
|------|-----------------|-----------|------|------|
| 智谱 GLM-4-flash | 0.1 | 0.02 | ✅ 国产 | — |
| DeepSeek V4 | 0.1 | 0.02 | ✅ 国产 | 第二层 |
| 通义千问 | 0.8 | 0.16 | ✅ 国产 | 第二层 |
| OpenAI GPT-4o-mini | 1.0 | 0.20 | ❌ 海外 | 第三层 |

## 决策

**选择智谱 GLM-4-flash 作为默认内置模型**，AI 调用成本包含在用户订阅费中。具体规则：

- MVP 阶段**只对接智谱 GLM-4-flash**，用户开箱即用，无需配置 API Key
- 代码层面通过 `AIModelProvider` trait 预留多模型扩展接口
- 设置页面提供模型选择 UI，但第二、三层模型标注「V1.0 自配 Key 启用」

## 理由

1. **成本最优**：¥0.02/次在所有候选模型中最低（与 DeepSeek V4 持平），个人版 500 次月成本仅 ¥10，毛利高达 66%
2. **国产合规**：智谱 AI 是国内厂商，数据不出境，满足金融、政务等行业合规要求
3. **OpenAI 兼容格式**：智谱 API 完全兼容 OpenAI Chat Completions 接口，切换模型中只需改 endpoint + model ID，代码改动量极小
4. **中文能力强**：GLM-4 系列在中文理解和生成任务上表现优异，适合「中文步骤说明」这一核心场景
5. **免费版成本可忽略**：20 次仅 ¥0.4，纯粹引流成本

## 后果

- **正面**：MVP 快速上线，AI 成本可控；单一模型降低初期开发复杂度和测试成本
- **负面**：用户无法在 MVP 阶段切换其他模型；若智谱服务不稳定，短期只能提示用户等待
- **缓解**：架构预留 `AIModelProvider` trait + 模型配置映射表，V1.0 可快速接入 DeepSeek / 通义千问
- **待跟进**：智谱 API SLA 监控、价格变动预警机制
*（内容由AI生成，仅供参考）*
