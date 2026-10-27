//! 人格系统的基础设施层。

pub mod sqlite_persona_store;

pub use sqlite_persona_store::{
    PERSONA_SCHEMA_IDENTITY, PersonaStore, PersonaStoreError, PublishedStyle, SettlementOutcome,
    StyleEntry, TransitionRecord, persona_store_path,
};
