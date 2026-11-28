/*
 * 语音配置 DTO：与 `[voice]` 段一一对应，读写共用。
 *
 * 单独成册的原因有两条：`types.ts` 受 700 行上限约束（tools/verify-architecture.ps1），
 * 而语音这段本身与模型管理同构，独立出来更好对照。
 */

/** 语音渠道 DTO：密钥只以变量名与是否已配置出现，绝无明文。 */
export interface VoiceChannelDto {
  name: string;
  enabled: boolean;
  protocol: string;
  baseUrl: string;
  apiKeyEnv: string | null;
  apiKeyConfigured: boolean;
  wireTransport: string;
  websocketUrl: string | null;
  priority: number;
  weight: number;
}

/** 语音模型 DTO。 */
export interface VoiceModelDto {
  name: string;
  channel: string;
  id: string;
  role: string;
  voice: string;
  speechRate: number;
  turnDetection: string;
  /** 模型优先级，数值越大越优先。 */
  priority: number;
  /** 同优先级模型的权重。 */
  weight: number;
}

/** 独立 `[voice]` 段 DTO，读写共用。 */
export interface VoiceConfigDto {
  channels: VoiceChannelDto[];
  models: VoiceModelDto[];
}
