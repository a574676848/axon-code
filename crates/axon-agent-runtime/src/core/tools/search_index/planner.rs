use super::{
    model::TrigramKey,
    trigram::{TRIGRAM_WIDTH, extract},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QueryPlan {
    MatchAll,
    /// 已排序去重的必需 trigram 集合。
    Required(Vec<TrigramKey>),
}

pub(crate) fn build_query_plan(query: &str, is_literal: bool, case_sensitive: bool) -> QueryPlan {
    if !is_literal || !case_sensitive || query.contains(['\n', '\r']) {
        return QueryPlan::MatchAll;
    }

    let bytes = query.as_bytes();
    if bytes.len() < TRIGRAM_WIDTH {
        return QueryPlan::MatchAll;
    }

    let required = extract(bytes);
    if required.is_empty() {
        QueryPlan::MatchAll
    } else {
        QueryPlan::Required(required)
    }
}

#[cfg(test)]
mod tests {
    use super::{QueryPlan, build_query_plan};

    #[test]
    fn literal_query_uses_required_trigrams() {
        assert!(matches!(
            build_query_plan("workspace", true, true),
            QueryPlan::Required(keys) if !keys.is_empty()
        ));
    }

    #[test]
    fn regex_and_case_insensitive_queries_fail_open() {
        assert_eq!(
            build_query_plan("workspace", false, true),
            QueryPlan::MatchAll
        );
        assert_eq!(
            build_query_plan("workspace", true, false),
            QueryPlan::MatchAll
        );
    }

    #[test]
    fn short_query_does_not_claim_a_candidate_reduction() {
        assert_eq!(build_query_plan("ab", true, true), QueryPlan::MatchAll);
    }
}
