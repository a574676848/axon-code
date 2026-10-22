//! 领域对象 ID。

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($name:ident) => {
        #[doc = concat!(stringify!($name), " 领域 ID。")]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        pub struct $name(pub u64);
    };
}

id_type!(AttachmentId);
id_type!(ByteChunkId);
id_type!(CellId);
id_type!(ExecutionRequestId);
id_type!(LongRunningJobId);
id_type!(MemoryCandidateId);
id_type!(MemoryId);
id_type!(ModelProfileId);
id_type!(OutputRefId);
id_type!(PaneId);
id_type!(PermissionSnapshotId);
id_type!(SessionId);
id_type!(SuggestionId);
id_type!(SuggestionUndoTokenId);
id_type!(TurnId);
id_type!(ToolCallId);
id_type!(ToolId);
