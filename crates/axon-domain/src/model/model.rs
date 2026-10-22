//! 模型路由领域对象。

use crate::ModelProfileId;

/// 默认单张图片字节上限。
pub const DEFAULT_VISION_MAX_BYTES_PER_IMAGE: u64 = 10 * 1024 * 1024;
/// 默认单次视觉请求总字节上限。
pub const DEFAULT_VISION_MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

/// 视觉输入限制。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisionInputLimits {
    /// 单张图片最大字节数。
    pub max_bytes_per_image: u64,
    /// 单次请求图片总字节上限。
    pub max_total_bytes: u64,
}

impl VisionInputLimits {
    /// 视觉能力关闭时的限制。
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            max_bytes_per_image: 0,
            max_total_bytes: 0,
        }
    }

    /// 创建标准图片输入限制。
    #[must_use]
    pub const fn standard_images() -> Self {
        Self {
            max_bytes_per_image: DEFAULT_VISION_MAX_BYTES_PER_IMAGE,
            max_total_bytes: DEFAULT_VISION_MAX_TOTAL_BYTES,
        }
    }

    /// 判断限制是否允许视觉输入。
    #[must_use]
    pub const fn allows_input(self) -> bool {
        self.max_bytes_per_image > 0 && self.max_total_bytes > 0
    }
}

/// 成本等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostClass {
    /// 低成本。
    Low,
    /// 中等成本。
    Medium,
    /// 高成本。
    High,
}

/// 模型画像。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelProfile {
    /// 模型 ID。
    pub id: ModelProfileId,
    /// provider 名称。
    pub provider: String,
    /// 模型名称。
    pub model: String,
    /// 上下文窗口。
    pub context_window: u64,
    /// 是否支持工具。
    pub supports_tools: bool,
    /// 是否支持视觉。
    pub supports_vision: bool,
    /// 视觉输入限制。
    pub vision_input_limits: VisionInputLimits,
    /// 支持的视觉输入 MIME。
    pub supported_input_mime_types: Vec<String>,
    /// 成本等级。
    pub cost_class: CostClass,
}

impl ModelProfile {
    /// 判断模型是否可接收指定 MIME 的视觉输入。
    #[must_use]
    pub fn supports_vision_mime(&self, mime_type: &str) -> bool {
        self.supports_vision
            && self.vision_input_limits.allows_input()
            && self
                .supported_input_mime_types
                .iter()
                .any(|supported| supported == mime_type)
    }
}

/// 任务类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// 解释命令输出。
    ExplainCommandOutput,
    /// 小型编辑。
    SmallEdit,
    /// 跨文件重构。
    CrossFileRefactor,
    /// 图片理解。
    ImageUnderstanding,
}

/// 路由意图：刻画任务在「推理能力」与「Token 成本」之间的取舍。
///
/// 与 `TaskKind` 正交：`TaskKind` 描述任务形态（编辑/重构/解释/视觉），
/// `RoutingObjective` 描述应当为之投入的算力档位，用于在成本与推理间权衡。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoutingObjective {
    /// 核心编排、校验、跨文件重构等高价值任务：优先最强推理模型，成本次之。
    DeepReasoning,
    /// 搜索、记忆检索、大批量阅读、代码修改等高频任务：优先低成本快速模型。
    FastBulk,
    /// 默认折中：沿用任务形态的层级偏好，兼顾质量与成本。
    #[default]
    Balanced,
}

/// 路由原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteReason {
    /// 机器可读编码。
    pub code: &'static str,
    /// 中文摘要。
    pub summary: String,
}

/// 任务分类结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskClassification {
    /// 任务类型。
    pub task_kind: TaskKind,
    /// 路由意图：推理能力与成本的取舍档位。
    pub objective: RoutingObjective,
    /// 是否需要工具。
    pub needs_tools: bool,
    /// 是否需要代码编辑。
    pub needs_code_edit: bool,
    /// 是否需要视觉能力。
    pub needs_vision: bool,
    /// 解释原因。
    pub reasons: Vec<RouteReason>,
}
