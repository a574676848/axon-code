/// 类型擦除的 prompt fragment 注册机制。
///
/// 用于 context 过滤代码识别注入的 fragment，无需构造具体的 context payload。
pub trait FragmentRegistration: Sync {
    /// 检查给定文本是否匹配此 fragment 类型。
    #[must_use]
    fn matches_text(&self, text: &str) -> bool;
}

/// Fragment 注册代理，用于类型擦除。
pub struct FragmentRegistrationProxy<T> {
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T> FragmentRegistrationProxy<T> {
    /// 创建新的 fragment 注册代理。
    #[must_use]
    pub const fn new() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> Default for FragmentRegistrationProxy<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: PromptFragment> FragmentRegistration for FragmentRegistrationProxy<T> {
    fn matches_text(&self, text: &str) -> bool {
        T::matches_text(text)
    }
}

/// Prompt fragment 所属 message role。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFragmentRole {
    /// System message。
    System,
    /// Developer message。
    Developer,
    /// User message。
    User,
}

impl PromptFragmentRole {
    /// 返回 provider message role 字符串。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Developer => "developer",
            Self::User => "user",
        }
    }
}

/// Prompt fragment 是否随产品版本稳定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFragmentStability {
    /// 稳定模板片段。
    Stable,
    /// 每轮动态片段。
    Dynamic,
}

/// Prompt fragment 生命周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFragmentLifecycle {
    /// 可进入 provider/cache 的稳定上下文。
    Cacheable,
    /// 仅本轮有效，不能被长期缓存。
    Ephemeral,
}

/// Prompt fragment 是否受预算裁剪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFragmentBudget {
    /// 参与预算控制。
    Budgeted,
    /// 不参与预算控制。
    Unbudgeted,
}

/// Prompt fragment 元数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptFragmentMetadata {
    /// message role。
    pub role: PromptFragmentRole,
    /// 稳定性。
    pub stability: PromptFragmentStability,
    /// 生命周期。
    pub lifecycle: PromptFragmentLifecycle,
    /// 预算属性。
    pub budget: PromptFragmentBudget,
}

impl PromptFragmentMetadata {
    /// 构造稳定的 system fragment。
    #[must_use]
    pub const fn stable_system() -> Self {
        Self {
            role: PromptFragmentRole::System,
            stability: PromptFragmentStability::Stable,
            lifecycle: PromptFragmentLifecycle::Cacheable,
            budget: PromptFragmentBudget::Budgeted,
        }
    }

    /// 构造稳定的 developer fragment。
    #[must_use]
    pub const fn stable_developer() -> Self {
        Self {
            role: PromptFragmentRole::Developer,
            stability: PromptFragmentStability::Stable,
            lifecycle: PromptFragmentLifecycle::Cacheable,
            budget: PromptFragmentBudget::Budgeted,
        }
    }

    /// 构造动态开发者片段。
    #[must_use]
    pub const fn dynamic_developer() -> Self {
        Self {
            role: PromptFragmentRole::Developer,
            stability: PromptFragmentStability::Dynamic,
            lifecycle: PromptFragmentLifecycle::Ephemeral,
            budget: PromptFragmentBudget::Budgeted,
        }
    }

    /// 构造动态且不参与上下文预算的开发者片段。
    #[must_use]
    pub const fn dynamic_unbudgeted_developer() -> Self {
        Self {
            role: PromptFragmentRole::Developer,
            stability: PromptFragmentStability::Dynamic,
            lifecycle: PromptFragmentLifecycle::Ephemeral,
            budget: PromptFragmentBudget::Unbudgeted,
        }
    }

    /// 构造动态但可缓存的 system fragment。
    #[must_use]
    pub const fn dynamic_cacheable_system() -> Self {
        Self {
            role: PromptFragmentRole::System,
            stability: PromptFragmentStability::Dynamic,
            lifecycle: PromptFragmentLifecycle::Cacheable,
            budget: PromptFragmentBudget::Budgeted,
        }
    }

    /// 构造动态但可缓存的 developer fragment。
    #[must_use]
    pub const fn dynamic_cacheable_developer() -> Self {
        Self {
            role: PromptFragmentRole::Developer,
            stability: PromptFragmentStability::Dynamic,
            lifecycle: PromptFragmentLifecycle::Cacheable,
            budget: PromptFragmentBudget::Budgeted,
        }
    }

    /// 构造动态用户片段。
    #[must_use]
    pub const fn dynamic_user() -> Self {
        Self {
            role: PromptFragmentRole::User,
            stability: PromptFragmentStability::Dynamic,
            lifecycle: PromptFragmentLifecycle::Ephemeral,
            budget: PromptFragmentBudget::Budgeted,
        }
    }

    /// 构造动态但可缓存的用户片段。
    #[must_use]
    pub const fn dynamic_cacheable_user() -> Self {
        Self {
            role: PromptFragmentRole::User,
            stability: PromptFragmentStability::Dynamic,
            lifecycle: PromptFragmentLifecycle::Cacheable,
            budget: PromptFragmentBudget::Budgeted,
        }
    }
}

/// Context payload 注入为 provider-visible prompt fragment。
///
/// 实现方拥有目标 message role 并提供精确的 fragment body。
/// 带标记的 fragment 提供 start/end markers 用于后续识别注入的 context。
/// `render()` 连接 markers 和 body 而不添加分隔符，因此实现应在 `body()` 中
/// 包含标签之间需要的任何空白。无标记的 fragment 应将两个 marker 留空，
/// 这种情况下默认 helper 仅渲染 body 且不匹配任意文本。
pub trait PromptFragment: std::fmt::Debug {
    /// 返回此 fragment 的元数据。
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::dynamic_user()
    }

    /// 返回此 fragment 的 role（如 "user"、"developer" 等）。
    fn role(&self) -> &'static str {
        self.metadata().role.as_str()
    }

    /// 返回此 fragment 的 start/end markers。
    fn markers(&self) -> (&'static str, &'static str);

    /// 返回此 fragment 的 body 内容。
    fn body(&self) -> String;

    /// 返回此 fragment 类型的静态 markers。
    fn type_markers() -> (&'static str, &'static str)
    where
        Self: Sized;

    /// 检查给定文本是否匹配此 fragment 类型。
    #[must_use]
    fn matches_text(text: &str) -> bool
    where
        Self: Sized,
    {
        let (start_marker, end_marker) = Self::type_markers();
        matches_marked_text(start_marker, end_marker, text)
    }

    /// 渲染完整的 fragment（包括 markers 和 body）。
    fn render(&self) -> String {
        let (start_marker, end_marker) = self.markers();
        let body = self.body();
        if start_marker.is_empty() && end_marker.is_empty() {
            return body;
        }

        format!("{start_marker}{body}{end_marker}")
    }
}

pub(crate) fn matches_marked_text(start_marker: &str, end_marker: &str, text: &str) -> bool {
    if start_marker.is_empty() || end_marker.is_empty() {
        return false;
    }

    let trimmed = text.trim_start();
    let starts_with_marker = trimmed
        .get(..start_marker.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(start_marker));
    let trimmed = trimmed.trim_end();
    let ends_with_marker = trimmed
        .get(trimmed.len().saturating_sub(end_marker.len())..)
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(end_marker));
    starts_with_marker && ends_with_marker
}
