//! 图片生成与编辑。
//!
//! 两类能力要分清:**文生图**(所有供应商都有)和**按指令编辑一张已有的图**(只有部分供应商有)。
//! 图片工作台里「持续对话修改」靠的是后者;没有编辑能力的供应商只能「改写提示词重新生成」,
//! 主体会变——所以 `can_edit()` 是接口的一部分,上层据此决定怎么做、怎么跟用户说。
//!
//! 适配器的共同约定(docs/04-integrations.md §4):
//! - 结果一律**立刻取回字节**再返回(各家的结果链接都是 24 小时过期);
//! - 显式关掉供应商水印(有的默认开);AI 生成标识由我们自己在资产上打标;
//! - 域名、模型名、单价都是配置,不是常量(MiniMax 换过域名,百炼的接入地址带工作空间)。

use async_trait::async_trait;
use base64::Engine as _;
use pp_common::imagery::ImageAspect;
use serde_json::{json, Value};

use crate::error::ProviderError;
use crate::http::{check_base_url, post_json};

#[derive(Debug, Clone, PartialEq)]
pub struct ImageRequest {
    pub prompt: String,
    pub aspect: ImageAspect,
    pub count: u32,
    /// 输入图(data URL)。非空 = 编辑 / 以图为条件生成;只有 `can_edit()` 的供应商接受
    pub images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageBytes {
    pub bytes: Vec<u8>,
    /// 小写扩展名,不带点
    pub ext: String,
}

#[async_trait]
pub trait ImageProvider: Send + Sync {
    /// 供应商标识(记在每张图上):minimax / qwen / demo
    fn name(&self) -> &'static str;
    fn model(&self) -> String;
    /// 能不能「给一张图 + 一句话,改出一张新图」
    fn can_edit(&self) -> bool;
    fn max_count(&self) -> u32;
    /// 每张图的价格(分)。是配置不是常量,这里给的是出厂预设
    fn price_fen(&self) -> f64;
    async fn generate(&self, req: &ImageRequest) -> Result<Vec<ImageBytes>, ProviderError>;
}

/// 魔数嗅探:供应商说是 png 的不一定是 png。
pub fn sniff_ext(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [0xFF, 0xD8, 0xFF, ..] => "jpg",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        _ => "png",
    }
}

fn check_request(req: &ImageRequest, provider: &dyn ImageProvider) -> Result<(), ProviderError> {
    if req.prompt.trim().is_empty() {
        return Err(ProviderError::BadResponse("image prompt is empty".into()));
    }
    if !req.images.is_empty() && !provider.can_edit() {
        return Err(ProviderError::BadResponse(format!("{} cannot edit images", provider.name())));
    }
    Ok(())
}

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, ProviderError> {
    let resp = client.get(url).send().await.map_err(|e| ProviderError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(ProviderError::Http {
            status,
            body: "could not download the generated image".into(),
        });
    }
    let bytes = resp.bytes().await.map_err(|e| ProviderError::Network(e.to_string()))?;
    Ok(bytes.to_vec())
}

// ---------------------------------------------------------------- MiniMax image-01

/// MiniMax `image-01`:便宜(约 ¥0.025 / 张)、快,**只能文生图**。
/// 接口 `POST {base}/v1/image_generation`(2026-09-21 核对 platform.minimax.cn 文档)。
pub struct MiniMaxImage {
    client: reqwest::Client,
    base_url: String,
    key: String,
    pub model: String,
}

impl MiniMaxImage {
    pub const DEFAULT_BASE: &'static str = "https://api.minimax.cn";

    pub fn new(client: reqwest::Client, base_url: &str, key: &str) -> Result<Self, ProviderError> {
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        check_base_url(&base_url)?;
        if key.trim().is_empty() {
            return Err(ProviderError::MissingKey);
        }
        Ok(Self {
            client,
            base_url,
            key: key.trim().to_string(),
            model: "image-01".into(),
        })
    }

    pub fn build_body(&self, req: &ImageRequest) -> Value {
        json!({
            "model": self.model,
            "prompt": req.prompt.chars().take(1500).collect::<String>(),
            "aspect_ratio": req.aspect.ratio(),
            "n": req.count.clamp(1, self.max_count()),
            // 直接要 base64:省一次下载,也不用操心 24 小时过期的链接
            "response_format": "base64",
            "prompt_optimizer": false,
            "aigc_watermark": false,
        })
    }

    /// 业务错误码在 `base_resp` 里,HTTP 状态可能仍是 200。
    pub fn parse_response(v: &Value) -> Result<Vec<ImageBytes>, ProviderError> {
        let code = v["base_resp"]["status_code"].as_i64().unwrap_or(0);
        if code != 0 {
            let msg = v["base_resp"]["status_msg"].as_str().unwrap_or("").to_string();
            return Err(match code {
                1004 | 2049 => ProviderError::Auth,
                1008 => ProviderError::InsufficientBalance,
                1002 | 1039 | 2045 => ProviderError::RateLimited,
                _ => ProviderError::BadResponse(format!("minimax {code}: {msg}")),
            });
        }
        let list = v["data"]["image_base64"]
            .as_array()
            .ok_or_else(|| ProviderError::BadResponse("minimax: no image_base64 in the response".into()))?;
        let mut out = Vec::new();
        for item in list {
            let Some(text) = item.as_str() else { continue };
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(text.trim())
                .map_err(|e| ProviderError::BadResponse(format!("minimax: bad base64: {e}")))?;
            out.push(ImageBytes {
                ext: sniff_ext(&bytes).to_string(),
                bytes,
            });
        }
        if out.is_empty() {
            // 整批被内容安全拦掉时 success_count = 0、列表为空
            return Err(ProviderError::BadResponse("minimax returned no images (the prompt may have been blocked)".into()));
        }
        Ok(out)
    }
}

#[async_trait]
impl ImageProvider for MiniMaxImage {
    fn name(&self) -> &'static str {
        "minimax"
    }
    fn model(&self) -> String {
        self.model.clone()
    }
    fn can_edit(&self) -> bool {
        false
    }
    fn max_count(&self) -> u32 {
        9
    }
    fn price_fen(&self) -> f64 {
        2.5
    }
    async fn generate(&self, req: &ImageRequest) -> Result<Vec<ImageBytes>, ProviderError> {
        check_request(req, self)?;
        let url = format!("{}/v1/image_generation", self.base_url);
        let v = post_json(&self.client, &url, &self.key, &self.build_body(req)).await?;
        Self::parse_response(&v)
    }
}

// ---------------------------------------------------------------- 通义千问图像(阿里云百炼)

/// 通义千问图像 `qwen-image-2.0`:生成与编辑一体,1~3 张输入图,一次最多 6 张。
/// 接口 `POST {base}/api/v1/services/aigc/multimodal-generation/generation`(2026-09-21 核对 help.aliyun.com 文档)。
/// 百炼现在给的接入地址带工作空间(`https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com`),所以 base 必须可配。
pub struct QwenImage {
    client: reqwest::Client,
    base_url: String,
    key: String,
    pub model: String,
}

impl QwenImage {
    pub const DEFAULT_BASE: &'static str = "https://dashscope.aliyuncs.com";

    pub fn new(client: reqwest::Client, base_url: &str, key: &str) -> Result<Self, ProviderError> {
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        check_base_url(&base_url)?;
        if key.trim().is_empty() {
            return Err(ProviderError::MissingKey);
        }
        Ok(Self {
            client,
            base_url,
            key: key.trim().to_string(),
            model: "qwen-image-2.0".into(),
        })
    }

    /// 总像素要在 512² ~ 2048² 之间;这几档都在 200 万像素上下。
    fn size(aspect: ImageAspect) -> &'static str {
        match aspect {
            ImageAspect::Square => "1440*1440",
            ImageAspect::Portrait => "1248*1664",
            ImageAspect::Landscape => "1664*1248",
            ImageAspect::Tall => "1080*1920",
            ImageAspect::Wide => "1920*1080",
        }
    }

    pub fn build_body(&self, req: &ImageRequest) -> Value {
        let mut content: Vec<Value> = req.images.iter().take(3).map(|img| json!({ "image": img })).collect();
        content.push(json!({ "text": req.prompt }));
        let mut parameters = json!({
            "n": req.count.clamp(1, self.max_count()),
            "watermark": false,
            // 提示词已经由我们的规划步骤写好了,不让服务端再改写一遍——改写会让「只改背景」这类指令跑偏
            "prompt_extend": false,
        });
        // 编辑时不给 size:输出比例跟随输入图,主体不会被重新构图
        if req.images.is_empty() {
            parameters["size"] = json!(Self::size(req.aspect));
        }
        json!({
            "model": self.model,
            "input": { "messages": [{ "role": "user", "content": content }] },
            "parameters": parameters,
        })
    }

    /// 取出结果图的链接。
    pub fn parse_response(v: &Value) -> Result<Vec<String>, ProviderError> {
        if let Some(code) = v["code"].as_str().filter(|c| !c.is_empty()) {
            let msg = v["message"].as_str().unwrap_or("");
            return Err(match code {
                "InvalidApiKey" => ProviderError::Auth,
                "Arrearage" => ProviderError::InsufficientBalance,
                "Throttling" | "Throttling.RateQuota" | "Throttling.AllocationQuota" => ProviderError::RateLimited,
                _ => ProviderError::BadResponse(format!("qwen {code}: {msg}")),
            });
        }
        let mut urls = Vec::new();
        for choice in v["output"]["choices"].as_array().into_iter().flatten() {
            for part in choice["message"]["content"].as_array().into_iter().flatten() {
                if let Some(url) = part["image"].as_str() {
                    urls.push(url.to_string());
                }
            }
        }
        if urls.is_empty() {
            return Err(ProviderError::BadResponse("qwen returned no images".into()));
        }
        Ok(urls)
    }
}

#[async_trait]
impl ImageProvider for QwenImage {
    fn name(&self) -> &'static str {
        "qwen"
    }
    fn model(&self) -> String {
        self.model.clone()
    }
    fn can_edit(&self) -> bool {
        true
    }
    fn max_count(&self) -> u32 {
        6
    }
    fn price_fen(&self) -> f64 {
        20.0
    }
    async fn generate(&self, req: &ImageRequest) -> Result<Vec<ImageBytes>, ProviderError> {
        check_request(req, self)?;
        let url = format!("{}/api/v1/services/aigc/multimodal-generation/generation", self.base_url);
        let v = post_json(&self.client, &url, &self.key, &self.build_body(req)).await?;
        let mut out = Vec::new();
        // 链接 24 小时过期:现在就取回来
        for link in Self::parse_response(&v)? {
            let bytes = download(&self.client, &link).await?;
            out.push(ImageBytes {
                ext: sniff_ext(&bytes).to_string(),
                bytes,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(images: Vec<String>) -> ImageRequest {
        ImageRequest {
            prompt: "白底,单个桌面线缆夹,3/4 视角,柔和棚拍光".into(),
            aspect: ImageAspect::Portrait,
            count: 4,
            images,
        }
    }

    fn client() -> reqwest::Client {
        crate::http::build_client(None).unwrap()
    }

    #[test]
    fn minimax_asks_for_base64_without_watermark_and_clamps_the_count() {
        let p = MiniMaxImage::new(client(), "https://api.minimax.cn/", "k").unwrap();
        let body = p.build_body(&ImageRequest { count: 40, ..req(vec![]) });
        assert_eq!(body["model"], "image-01");
        assert_eq!(body["aspect_ratio"], "3:4");
        assert_eq!(body["n"], 9);
        assert_eq!(body["response_format"], "base64");
        assert_eq!(body["aigc_watermark"], false);
        assert!(!p.can_edit());
    }

    #[test]
    fn minimax_business_errors_hide_behind_http_200() {
        let ok = json!({"data": {"image_base64": ["iVBORw0KGgo="]}, "base_resp": {"status_code": 0}});
        let images = MiniMaxImage::parse_response(&ok).unwrap();
        assert_eq!((images.len(), images[0].ext.as_str()), (1, "png"));

        let auth = json!({"base_resp": {"status_code": 1004, "status_msg": "login fail"}});
        assert_eq!(MiniMaxImage::parse_response(&auth).unwrap_err(), ProviderError::Auth);
        let broke = json!({"base_resp": {"status_code": 1008, "status_msg": "insufficient balance"}});
        assert_eq!(MiniMaxImage::parse_response(&broke).unwrap_err(), ProviderError::InsufficientBalance);
        let blocked = json!({"data": {"image_base64": []}, "metadata": {"success_count": 0}, "base_resp": {"status_code": 0}});
        assert!(matches!(MiniMaxImage::parse_response(&blocked), Err(ProviderError::BadResponse(m)) if m.contains("no images")));
    }

    #[test]
    fn qwen_puts_input_images_before_the_instruction_and_lets_edits_keep_their_framing() {
        let p = QwenImage::new(client(), QwenImage::DEFAULT_BASE, "k").unwrap();
        let generate = p.build_body(&req(vec![]));
        assert_eq!(generate["parameters"]["size"], "1248*1664");
        assert_eq!(generate["parameters"]["watermark"], false);
        assert_eq!(generate["input"]["messages"][0]["content"].as_array().unwrap().len(), 1);

        let edit = p.build_body(&req(vec!["data:image/jpeg;base64,AAAA".into(); 5]));
        let content = edit["input"]["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 4, "最多 3 张输入图 + 1 段文字");
        assert!(content[0]["image"].as_str().unwrap().starts_with("data:image/") && content[3]["text"].is_string());
        assert!(edit["parameters"].get("size").is_none(), "编辑时比例跟随输入图");
        assert_eq!(edit["parameters"]["n"], 4);
        assert!(p.can_edit());
    }

    #[test]
    fn qwen_results_and_errors_are_read_from_their_own_shapes() {
        let ok = json!({"output": {"choices": [{"message": {"content": [{"image": "https://x/a.png"}, {"image": "https://x/b.png"}]}}]}});
        assert_eq!(QwenImage::parse_response(&ok).unwrap(), ["https://x/a.png", "https://x/b.png"]);
        let bad_key = json!({"code": "InvalidApiKey", "message": "Invalid API-key provided."});
        assert_eq!(QwenImage::parse_response(&bad_key).unwrap_err(), ProviderError::Auth);
        assert!(QwenImage::parse_response(&json!({"output": {"choices": []}})).is_err());
    }

    #[test]
    fn a_workspace_specific_endpoint_is_accepted_but_plain_http_is_not() {
        assert!(QwenImage::new(client(), "https://ws-123.cn-beijing.maas.aliyuncs.com", "k").is_ok());
        assert!(QwenImage::new(client(), "http://dashscope.aliyuncs.com", "k").is_err());
        assert_eq!(MiniMaxImage::new(client(), MiniMaxImage::DEFAULT_BASE, "  ").err(), Some(ProviderError::MissingKey));
    }

    #[test]
    fn image_types_are_sniffed_from_the_bytes() {
        assert_eq!(sniff_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), "jpg");
        assert_eq!(sniff_ext(b"\x89PNG\r\n\x1a\n"), "png");
        assert_eq!(sniff_ext(b"RIFF\x00\x00\x00\x00WEBPVP8 "), "webp");
    }

    #[tokio::test]
    async fn a_generate_only_provider_refuses_edit_requests_before_spending_anything() {
        let p = MiniMaxImage::new(client(), MiniMaxImage::DEFAULT_BASE, "k").unwrap();
        let err = p.generate(&req(vec!["data:image/png;base64,AA".into()])).await.unwrap_err();
        assert!(matches!(err, ProviderError::BadResponse(m) if m.contains("cannot edit")));
    }
}
