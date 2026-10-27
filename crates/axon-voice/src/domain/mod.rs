//! 领域层：相位、失败、画像与快照。

pub mod cue;
pub mod failure;
pub mod phase;
pub mod profile;
pub mod snapshot;

pub use cue::CueKind;
pub use failure::{
    IdlePhase, ModelFamily, PresetReason, ProtocolReason, Stage, VoiceFailure, WorkerSubStage,
};
pub use phase::{Phase, PhaseError, TransitionTrigger};
pub use profile::{ProfileError, ProfileSource, SpeechProfile, Verbosity};
pub use snapshot::{
    HumorLevel, PersonaDirective, PersonaMode, PersonaSnapshot, PersonaStateView, UserAffect,
};
