//! 人格系统领域层。

pub mod expression_policy;
pub mod persona_core;
pub mod persona_event;
pub mod persona_state;
pub mod scene_frame;
pub mod soul_document;
pub mod state_machine;
pub mod voice;

pub use expression_policy::{ExpressionDirective, hides_affect, resolve_expression};
pub use persona_core::{Identity, PersonaCore, RuleCondition, SceneRule};
pub use persona_event::{PersonaEvent, TurnFinalStatus, TurnSettledInput};
pub use persona_state::{PersonaState, UserAffect};
pub use scene_frame::{Audience, Channel, Daypart, PersonaMode, SceneFrame, resolve_mode};
pub use soul_document::{SoulDocument, SoulError};
pub use state_machine::{PersonaError, Transition, transition};
pub use voice::{HumorLevel, Verbosity, Voice, VoicePatch};
