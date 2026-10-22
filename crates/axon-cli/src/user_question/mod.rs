//! 用户问答交互宿主模块（AXON-UQ-001）。

pub mod answer_route;
pub mod answer_service;
pub mod content;
pub mod fanout;
pub mod host;
pub mod http_adapter;
pub mod lane_steerer;

pub use answer_route::{AnswerRouteOutcome, parse_answer_body, route_answer};
pub use answer_service::{
    AnswerError, NoopQuestionMessageSteerer, QuestionMessageSteerer, UserQuestionAnswerService,
};
pub use content::{
    QUESTION_RECEIPT_BLOCK_HEADER, QUESTION_RECEIPT_CALL_PREFIX, QUESTION_RECEIPT_CONTENT_PREFIX,
    encode_question_answer_message, extract_question_receipt, format_answer_summary,
    strip_question_receipt_block,
};
pub use fanout::UserQuestionFanoutSink;
pub use host::{
    NoopUserQuestionEventSink, QuestionAnswerDelivery, UserQuestionEventSink, UserQuestionHost,
};
/// 车道答案注入器只在本 crate 内使用：它是答案服务的内部零件，不对外暴露。
pub(crate) use lane_steerer::LaneQuestionMessageSteerer;
