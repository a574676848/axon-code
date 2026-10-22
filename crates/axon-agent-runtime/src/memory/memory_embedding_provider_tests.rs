//! Memory embedding provider 合同测试。

use crate::memory_embedding_provider::*;
use axon_memory::MemoryEmbeddingCredentialPlan;

const EXPECTED_DIMENSION: usize = 3;

#[test]
fn memory_embedding_plan_is_openai_compatible_and_remote_closed() {
    let plan = MemoryEmbeddingProviderRuntime
        .plan_openai(&MemoryEmbeddingRequestInput {
            provider_protocol: MEMORY_EMBEDDING_PROVIDER_OPENAI.to_owned(),
            base_url: "https://memory.example/v1".to_owned(),
            model_id: "text-embedding".to_owned(),
            input_text: "检索当前任务".to_owned(),
            api_key_env: Some("MEMORY_API_KEY".to_owned()),
            api_key: None,
            expected_dimension: EXPECTED_DIMENSION,
        })
        .unwrap();

    assert_eq!(plan.url, "https://memory.example/v1/embeddings");
    assert!(plan.body_text.contains("\"model\":\"text-embedding\""));
    assert!(plan.body_text.contains("检索当前任务"));
    assert!(!plan.remote_request_body_allowed);
    assert!(!plan.remote_raw_response_allowed);
    assert_eq!(
        plan.auth_header.as_ref().map(|header| &header.credential),
        Some(&MemoryEmbeddingCredentialPlan::Env {
            name: "MEMORY_API_KEY".to_owned()
        })
    );
}

#[test]
fn memory_embedding_response_validates_vector_shape_and_values() {
    let runtime = MemoryEmbeddingProviderRuntime;
    let vector = runtime
        .parse_openai_response(
            r#"{"data":[{"embedding":[1.0,0.5,-0.25]}]}"#,
            EXPECTED_DIMENSION,
        )
        .unwrap();
    assert_eq!(vector, vec![1.0, 0.5, -0.25]);

    assert!(
        runtime
            .parse_openai_response(r#"{"data":[{"embedding":[1.0]}]}"#, EXPECTED_DIMENSION)
            .is_err()
    );
    assert!(
        runtime
            .parse_openai_response(
                r#"{"data":[{"embedding":[0.0,0.0,0.0]}]}"#,
                EXPECTED_DIMENSION,
            )
            .is_err()
    );
}

#[test]
fn memory_embedding_response_accepts_dashscope_multimodal_output() {
    let vector = MemoryEmbeddingProviderRuntime
        .parse_openai_response(
            r#"{"output":{"embeddings":[{"embedding":[0.25,-0.5,0.75],"text_index":0}]},"usage":{"input_tokens":5},"request_id":"request-id"}"#,
            EXPECTED_DIMENSION,
        )
        .unwrap();

    assert_eq!(vector, vec![0.25, -0.5, 0.75]);
}

#[test]
fn memory_embedding_plan_covers_bailian_and_ark_compatible_bodies() {
    let runtime = MemoryEmbeddingProviderRuntime;
    let bailian = runtime
        .plan_provider(&MemoryEmbeddingRequestInput {
            provider_protocol: MEMORY_EMBEDDING_PROVIDER_OPENAI_CHAT.to_owned(),
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_owned(),
            model_id: "text-embedding-v4".to_owned(),
            input_text: "检索百炼".to_owned(),
            api_key_env: Some("DASHSCOPE_API_KEY".to_owned()),
            api_key: None,
            expected_dimension: 1_536,
        })
        .unwrap();
    assert_eq!(
        bailian.url,
        "https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings"
    );
    let bailian_body: serde_json::Value = serde_json::from_str(&bailian.body_text).unwrap();
    assert_eq!(bailian_body["model"], "text-embedding-v4");
    assert_eq!(bailian_body["input"], "检索百炼");
    // OpenAI-compatible 格式不包含 dimensions 字段
    assert!(bailian_body.get("dimensions").is_none());
    assert!(bailian_body.get("dimension").is_none());
    assert_eq!(
        bailian
            .auth_header
            .as_ref()
            .map(|header| &header.credential),
        Some(&MemoryEmbeddingCredentialPlan::Env {
            name: "DASHSCOPE_API_KEY".to_owned()
        })
    );

    let ark = runtime
        .plan_provider(&MemoryEmbeddingRequestInput {
            provider_protocol: MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL.to_owned(),
            base_url: "https://ark.cn-beijing.volces.com/api/v3".to_owned(),
            model_id: "doubao-embedding-vision-251215".to_owned(),
            input_text: "检索方舟".to_owned(),
            api_key_env: Some("ARK_API_KEY".to_owned()),
            api_key: None,
            expected_dimension: 2_048,
        })
        .unwrap();
    assert_eq!(
        ark.url,
        "https://ark.cn-beijing.volces.com/api/v3/embeddings/multimodal"
    );
    let ark_body: serde_json::Value = serde_json::from_str(&ark.body_text).unwrap();
    assert_eq!(ark_body["model"], "doubao-embedding-vision-251215");
    assert_eq!(ark_body["input"][0]["type"], "text");
    assert_eq!(ark_body["input"][0]["text"], "检索方舟");
    assert_eq!(ark_body["encoding_format"], "float");
    assert_eq!(ark_body["dimensions"], 2_048);
    assert!(!ark.remote_request_body_allowed);
    assert!(!ark.remote_raw_response_allowed);
}

#[test]
fn memory_embedding_plan_supports_bailian_multimodal_text_query() {
    let plan = MemoryEmbeddingProviderRuntime
        .plan_provider(&MemoryEmbeddingRequestInput {
            provider_protocol: MEMORY_EMBEDDING_PROVIDER_DASHSCOPE.to_owned(),
            base_url: "https://dashscope.aliyuncs.com/api/v1".to_owned(),
            model_id: "qwen3-vl-embedding".to_owned(),
            input_text: "检索图片资料".to_owned(),
            api_key_env: Some("DASHSCOPE_API_KEY".to_owned()),
            api_key: None,
            expected_dimension: 1_024,
        })
        .unwrap();

    assert_eq!(
        plan.url,
        "https://dashscope.aliyuncs.com/api/v1/services/embeddings/multimodal-embedding/multimodal-embedding"
    );
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();
    assert_eq!(body["model"], "qwen3-vl-embedding");
    assert_eq!(body["input"]["contents"][0]["text"], "检索图片资料");
    assert_eq!(body["parameters"]["dimension"], 1_024);
}

#[test]
fn memory_embedding_plan_covers_bailian_and_ark_visual_inputs() {
    let runtime = MemoryEmbeddingProviderRuntime;
    let bailian = runtime
        .plan_multimodal_provider(&MemoryEmbeddingMultimodalRequestInput {
            provider_protocol: MEMORY_EMBEDDING_PROVIDER_DASHSCOPE.to_owned(),
            base_url: "https://dashscope.aliyuncs.com/api/v1".to_owned(),
            model_id: "qwen3-vl-embedding".to_owned(),
            contents: vec![
                MemoryEmbeddingMultimodalContent {
                    text: Some("找相似商品".to_owned()),
                    image_url: Some("https://example.com/product.png".to_owned()),
                    video_url: None,
                },
                MemoryEmbeddingMultimodalContent {
                    text: None,
                    image_url: None,
                    video_url: Some("https://example.com/demo.mp4".to_owned()),
                },
            ],
            api_key_env: Some("DASHSCOPE_API_KEY".to_owned()),
            api_key: None,
            expected_dimension: 1_024,
            dashscope_enable_fusion: true,
        })
        .unwrap();
    assert_eq!(
        bailian.url,
        "https://dashscope.aliyuncs.com/api/v1/services/embeddings/multimodal-embedding/multimodal-embedding"
    );
    let bailian_body: serde_json::Value = serde_json::from_str(&bailian.body_text).unwrap();
    assert_eq!(bailian_body["model"], "qwen3-vl-embedding");
    assert_eq!(bailian_body["input"]["contents"][0]["text"], "找相似商品");
    assert_eq!(
        bailian_body["input"]["contents"][0]["image"],
        "https://example.com/product.png"
    );
    assert_eq!(
        bailian_body["input"]["contents"][1]["video"],
        "https://example.com/demo.mp4"
    );
    assert_eq!(bailian_body["parameters"]["dimension"], 1_024);
    assert_eq!(bailian_body["parameters"]["enable_fusion"], true);
    assert!(!bailian.remote_request_body_allowed);
    assert!(!bailian.remote_raw_response_allowed);

    let ark = runtime
        .plan_multimodal_provider(&MemoryEmbeddingMultimodalRequestInput {
            provider_protocol: MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL.to_owned(),
            base_url: "https://ark.cn-beijing.volces.com/api/v3".to_owned(),
            model_id: "doubao-embedding-vision-251215".to_owned(),
            contents: vec![MemoryEmbeddingMultimodalContent {
                text: Some("视频和图片里有什么".to_owned()),
                image_url: Some("https://example.com/tower.png".to_owned()),
                video_url: Some("https://example.com/input.mp4".to_owned()),
            }],
            api_key_env: Some("ARK_API_KEY".to_owned()),
            api_key: None,
            expected_dimension: 2_048,
            dashscope_enable_fusion: false,
        })
        .unwrap();
    let ark_body: serde_json::Value = serde_json::from_str(&ark.body_text).unwrap();
    assert_eq!(ark_body["input"][0]["type"], "text");
    assert_eq!(ark_body["input"][1]["type"], "image_url");
    assert_eq!(
        ark_body["input"][1]["image_url"]["url"],
        "https://example.com/tower.png"
    );
    assert_eq!(ark_body["input"][2]["type"], "video_url");
    assert_eq!(
        ark_body["input"][2]["video_url"]["url"],
        "https://example.com/input.mp4"
    );
}

#[test]
fn memory_embedding_visual_inputs_fail_closed_for_openai_provider() {
    let runtime = MemoryEmbeddingProviderRuntime;
    // OpenAI 的纯文本 embedding 不支持多模态
    let input = MemoryEmbeddingMultimodalRequestInput {
        provider_protocol: MEMORY_EMBEDDING_PROVIDER_OPENAI.to_owned(),
        base_url: "https://api.openai.com/v1".to_owned(),
        model_id: "text-embedding-3-small".to_owned(),
        contents: vec![MemoryEmbeddingMultimodalContent {
            text: Some("找图".to_owned()),
            image_url: Some("https://example.com/image.png".to_owned()),
            video_url: None,
        }],
        api_key_env: Some("OPENAI_API_KEY".to_owned()),
        api_key: None,
        expected_dimension: EXPECTED_DIMENSION,
        dashscope_enable_fusion: false,
    };
    assert!(runtime.plan_multimodal_provider(&input).is_err());
}

#[test]
fn memory_embedding_supports_base64_images() {
    let runtime = MemoryEmbeddingProviderRuntime;
    let base64_input = MemoryEmbeddingMultimodalRequestInput {
        provider_protocol: MEMORY_EMBEDDING_PROVIDER_DASHSCOPE.to_owned(),
        base_url: "https://dashscope.aliyuncs.com/api/v1".to_owned(),
        model_id: "qwen3-vl-embedding".to_owned(),
        contents: vec![MemoryEmbeddingMultimodalContent {
            text: Some("base64 图片".to_owned()),
            image_url: Some("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==".to_owned()),
            video_url: None,
        }],
        api_key_env: Some("DASHSCOPE_API_KEY".to_owned()),
        api_key: None,
        expected_dimension: EXPECTED_DIMENSION,
        dashscope_enable_fusion: false,
    };
    let plan = runtime.plan_multimodal_provider(&base64_input).unwrap();
    assert!(plan.body_text.contains("data:image/png;base64"));
}

#[test]
fn memory_embedding_response_accepts_ark_multimodal_data_object() {
    let vector = MemoryEmbeddingProviderRuntime
        .parse_openai_response(
            r#"{"data":{"embedding":[0.25,-0.5,0.75],"object":"embedding"},"object":"list"}"#,
            EXPECTED_DIMENSION,
        )
        .unwrap();
    assert_eq!(vector, vec![0.25, -0.5, 0.75]);
}
