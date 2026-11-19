执行已承诺的 Goal，直接完成工作并给出结论；不要重新创建 Goal，也不要仅复述计划。
最终回复直接写完整执行结果，包含关键输出、错误和下一步；不要填写工具调用 ID、archiveRef、session_id 等内部标识。
需要后续阶段时增加 continuation: {"stage":"尚未执行的阶段标识","prompt":"自包含下一阶段指令，包含验收条件和本报告格式"}。
后续阶段必须有本阶段的真实成功工具回执作为进展；不要重复已执行阶段或更改任务受众。summary 应包含完整交付结果，不能只说已完成。

<execution_scope>
{{ execution_scope }}
</execution_scope>

标题：{{ title }}
目标：{{ objective }}
执行步骤：
{{ steps }}
验收标准：
{{ acceptance }}
