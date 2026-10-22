//! JARVIS Activity 查询应用服务。
//!
//! Activity 直接读取 Work Ledger 事实源，不维护 Projection 或独立缓存。

use std::fmt;

pub use axon_domain::{ActivityQuery, ActivitySnapshot};

use crate::ports::{
    GoalStore, GoalStoreError, LedgerEventStore, LedgerEventStoreError, ResultStore,
    ResultStoreError,
};

/// Activity 查询错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityServiceError {
    /// Goal 读取失败。
    Goal(GoalStoreError),
    /// Result 读取失败。
    Result(ResultStoreError),
    /// Event 读取失败。
    Event(LedgerEventStoreError),
}

impl fmt::Display for ActivityServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Goal(error) => write!(formatter, "Activity Goal 查询失败：{error}"),
            Self::Result(error) => write!(formatter, "Activity Result 查询失败：{error}"),
            Self::Event(error) => write!(formatter, "Activity Event 查询失败：{error}"),
        }
    }
}

impl std::error::Error for ActivityServiceError {}

/// 直接从事实存储查询 Activity 的服务。
#[derive(Debug, Clone)]
pub struct ActivityService<S> {
    store: S,
}

impl<S> ActivityService<S>
where
    S: GoalStore + ResultStore + LedgerEventStore,
{
    /// 创建 Activity 查询服务。
    #[must_use]
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// 按查询参数有界读取 Goal、Result 和 Event。
    pub fn read(&self, query: &ActivityQuery) -> Result<ActivitySnapshot, ActivityServiceError> {
        let active_goals = self
            .store
            .list_active_goals_for_scope(&query.scope, query.goal_limit)
            .map_err(ActivityServiceError::Goal)?;
        let unread_results = self
            .store
            .list_unread_results(&query.scope, query.result_limit)
            .map_err(ActivityServiceError::Result)?;
        let events = self
            .store
            .replay_ledger_events(query.event_after_sequence, query.event_limit)
            .map_err(ActivityServiceError::Event)?;
        Ok(ActivitySnapshot {
            active_goals,
            unread_results,
            events,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axon_domain::{
        ExecutionId, GoalId, GoalRecord, JarvisEvent, ResultId, ResultRecord, ScopeRef,
    };

    use super::{ActivityQuery, ActivityService};
    use crate::ports::{
        GoalStore, GoalStoreError, LedgerEventStore, LedgerEventStoreError, ResultStore,
        ResultStoreError,
    };

    #[derive(Clone, Default)]
    struct RecordingStore {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl GoalStore for RecordingStore {
        fn create_goal(&self, _: &GoalRecord) -> Result<bool, GoalStoreError> {
            Ok(false)
        }

        fn get_goal(&self, _: &GoalId) -> Result<Option<GoalRecord>, GoalStoreError> {
            Ok(None)
        }

        fn list_active_goals(&self, _: u32) -> Result<Vec<GoalRecord>, GoalStoreError> {
            Ok(Vec::new())
        }

        fn list_active_goals_for_scope(
            &self,
            scope: &ScopeRef,
            limit: u32,
        ) -> Result<Vec<GoalRecord>, GoalStoreError> {
            self.calls
                .lock()
                .expect("calls lock")
                .push(format!("goal:{scope:?}:{limit}"));
            Ok(Vec::new())
        }

        fn update_goal(&self, _: &GoalRecord, _: u64) -> Result<(), GoalStoreError> {
            Ok(())
        }
    }

    impl ResultStore for RecordingStore {
        fn list_unread_results(
            &self,
            scope: &ScopeRef,
            limit: u32,
        ) -> Result<Vec<ResultRecord>, ResultStoreError> {
            self.calls
                .lock()
                .expect("calls lock")
                .push(format!("result:{scope:?}:{limit}"));
            Ok(Vec::new())
        }

        fn acknowledge_results(
            &self,
            _: &ScopeRef,
            _: &[ResultId],
            _: u64,
        ) -> Result<u32, ResultStoreError> {
            Ok(0)
        }

        fn result_for_execution(
            &self,
            execution_id: &ExecutionId,
        ) -> Result<Option<ResultRecord>, ResultStoreError> {
            self.calls
                .lock()
                .expect("calls lock")
                .push(format!("result-for-execution:{execution_id}"));
            Ok(None)
        }

        fn expire_results(
            &self,
            _: &ScopeRef,
            _: &[ResultId],
            _: u64,
        ) -> Result<u32, ResultStoreError> {
            Ok(0)
        }
    }

    impl LedgerEventStore for RecordingStore {
        fn append_ledger_event(&self, _: &JarvisEvent) -> Result<bool, LedgerEventStoreError> {
            Ok(false)
        }

        fn replay_ledger_events(
            &self,
            after_sequence: u64,
            limit: u32,
        ) -> Result<Vec<JarvisEvent>, LedgerEventStoreError> {
            self.calls
                .lock()
                .expect("calls lock")
                .push(format!("event:{after_sequence}:{limit}"));
            Ok(Vec::new())
        }
    }

    #[test]
    fn activity_reads_each_fact_source_with_explicit_bounds() {
        let store = RecordingStore::default();
        let calls = store.calls.clone();
        let service = ActivityService::new(store);
        let query = ActivityQuery {
            scope: ScopeRef::global(),
            goal_limit: 3,
            result_limit: 5,
            event_after_sequence: 8,
            event_limit: 13,
        };

        let snapshot = service.read(&query).expect("activity");

        assert!(snapshot.active_goals.is_empty());
        assert!(snapshot.unread_results.is_empty());
        assert!(snapshot.events.is_empty());
        let calls = calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 3);
        assert!(calls[0].contains("goal:"));
        assert!(calls[0].ends_with(":3"));
        assert!(calls[1].contains("result:"));
        assert!(calls[1].ends_with(":5"));
        assert_eq!(calls[2], "event:8:13");
    }
}
