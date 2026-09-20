//! 接口密钥与供应商连通性。
//!
//! 硬约束(CLAUDE.md 4):密钥只进系统凭据管理器;不进日志;前端只知道 `has_key`。

use std::sync::Arc;
use std::time::Instant;

use pp_common::errcode;
use pp_common::provider::{ProviderId, ProviderStatus};
use pp_providers::llm::{ChatMessage, ChatRequest, LlmProvider, OpenAiCompat};
use pp_providers::search::{SearchProvider, SearchQuery, ZhipuSearch};
use pp_providers::ProviderError;
use tauri::State;

use super::join_err;
use crate::credentials::entry_name;
use crate::AppCtx;

pub(crate) fn provider_err(e: ProviderError) -> String {
    errcode::err(e.code(), e)
}

fn keyring_err(e: String) -> String {
    errcode::err(errcode::KEYRING_FAILED, e)
}

/// 取某个供应商的密钥;没配返回 `None`。
pub(crate) fn key_of(ctx: &AppCtx, id: ProviderId) -> Result<Option<String>, String> {
    ctx.credentials.get(&entry_name(id)).map_err(keyring_err)
}

pub(crate) fn llm_for(ctx: &AppCtx) -> Result<OpenAiCompat, String> {
    let key = key_of(ctx, ProviderId::Deepseek)?.ok_or_else(|| provider_err(ProviderError::MissingKey))?;
    let client = pp_providers::http::build_client(None).map_err(provider_err)?;
    OpenAiCompat::deepseek(client, &key).map_err(provider_err)
}

/// 搜索是可选能力:没配密钥返回 `Ok(None)`,调研流水线会走降级。
pub(crate) fn search_for(ctx: &AppCtx) -> Result<Option<ZhipuSearch>, String> {
    let Some(key) = key_of(ctx, ProviderId::ZhipuSearch)? else {
        return Ok(None);
    };
    let client = pp_providers::http::build_client(None).map_err(provider_err)?;
    ZhipuSearch::new(client, &key).map(Some).map_err(provider_err)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn provider_status(ctx: State<'_, Arc<AppCtx>>) -> Result<Vec<ProviderStatus>, String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        ProviderId::ALL
            .into_iter()
            .map(|id| {
                Ok(ProviderStatus {
                    id,
                    has_key: key_of(&ctx, id)?.is_some(),
                })
            })
            .collect()
    })
    .await
    .map_err(join_err)?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_provider_key(ctx: State<'_, Arc<AppCtx>>, id: ProviderId, key: String) -> Result<(), String> {
    let key = key.trim().to_string();
    if key.is_empty() || key.chars().any(char::is_whitespace) {
        return Err(errcode::err(errcode::INVALID_INPUT, "api key is empty or contains whitespace"));
    }
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        ctx.credentials.save(&entry_name(id), &key).map_err(keyring_err)?;
        log::info!("[provider] 已保存 {} 的密钥(长度 {})", id.as_str(), key.len());
        Ok(())
    })
    .await
    .map_err(join_err)?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_provider_key(ctx: State<'_, Arc<AppCtx>>, id: ProviderId) -> Result<(), String> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        ctx.credentials.delete(&entry_name(id)).map_err(keyring_err)?;
        log::info!("[provider] 已删除 {} 的密钥", id.as_str());
        Ok(())
    })
    .await
    .map_err(join_err)?
}

/// 「测试连接」:发一个最小的真实请求,返回耗时(毫秒)。会产生极小的费用(搜索一次约 1 分钱)。
#[tauri::command(rename_all = "snake_case")]
pub async fn test_provider(ctx: State<'_, Arc<AppCtx>>, id: ProviderId) -> Result<u64, String> {
    let ctx = ctx.inner().clone();
    let started = Instant::now();
    match id {
        ProviderId::Deepseek => {
            let llm = llm_for(&ctx)?;
            let req = ChatRequest::new("deepseek-flash", vec![ChatMessage::user("回复两个字:收到")]).max_tokens(16);
            llm.chat(req).await.map_err(provider_err)?;
        }
        ProviderId::ZhipuSearch => {
            let search = search_for(&ctx)?.ok_or_else(|| provider_err(ProviderError::MissingKey))?;
            let mut q = SearchQuery::new("3D打印");
            q.count = 1;
            search.search(q).await.map_err(provider_err)?;
        }
    }
    let ms = started.elapsed().as_millis() as u64;
    log::info!("[provider] {} 连接正常,{ms}ms", id.as_str());
    Ok(ms)
}
