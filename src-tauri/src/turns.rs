//! 正在跑的「一轮」(建模对话、出图、调研)的登记处,用来**中途取消**。
//!
//! 每一轮有一个作用域键(`design:<id>` / `board:<id>` / `research`)。前端点「停止」→ `cancel_turn(scope)`:
//! - 等着模型 / 图片接口回答的那个 future 被丢掉 → 连接随之关闭,供应商那边停止生成;
//! - 建模引擎里正在执行的脚本进程被杀掉(`CancelFlag`,约 0.1 秒内生效)。
//!
//! 和「一轮要么整轮入库,要么什么都不留」的约定配套:**只有花时间的那一段可以取消**(调模型、出图、跑脚本);
//! 一旦开始落盘入库就不再理会取消——否则会出现「界面说取消了,库里却多了半轮」的事。
//! 已经生成的 token / 图片,供应商照样计费;取消省下的是还没生成的那部分。

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use pp_cad::CancelFlag;
use pp_common::errcode;
use tokio::sync::watch;

#[derive(Default)]
pub struct Turns {
    running: Mutex<HashMap<String, (watch::Sender<bool>, CancelFlag)>>,
}

/// 一轮的「取消句柄」。这一轮结束(无论成败、还是被取消)时自动注销。
pub struct TurnGuard {
    key: String,
    turns: Arc<Turns>,
    rx: watch::Receiver<bool>,
    /// 交给建模引擎的执行器:取消时它负责杀掉正在跑的脚本
    pub flag: CancelFlag,
}

pub fn cancelled() -> String {
    errcode::err(errcode::CANCELLED, "cancelled by user")
}

impl Turns {
    /// 登记一轮。同一个作用域已经有一轮在跑 → 拒绝(界面上本来也不让连发两句;这里是兜底)。
    pub fn begin(self: &Arc<Self>, key: &str) -> Result<TurnGuard, String> {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if running.contains_key(key) {
            return Err(errcode::err(errcode::INVALID_INPUT, format!("a turn is already running for {key}")));
        }
        let (tx, rx) = watch::channel(false);
        let flag = CancelFlag::default();
        running.insert(key.to_string(), (tx, flag.clone()));
        Ok(TurnGuard {
            key: key.to_string(),
            turns: self.clone(),
            rx,
            flag,
        })
    }

    /// 取消一轮。没有这样一轮在跑(已经结束了)→ false,不算错。
    pub fn cancel(&self, key: &str) -> bool {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        match running.get(key) {
            Some((tx, flag)) => {
                flag.cancel();
                let _ = tx.send(true);
                true
            }
            None => false,
        }
    }
}

impl TurnGuard {
    /// 跑 `work`;中途被取消就立刻返回 `#cancelled#…`,`work` 这个 future 随之被丢掉。
    /// 取消发生在开始之前也算数(先点了停止、这一段才开始)。
    pub async fn run<T>(&self, work: impl Future<Output = Result<T, String>>) -> Result<T, String> {
        let mut rx = self.rx.clone();
        tokio::select! {
            biased;
            _ = rx.wait_for(|stop| *stop) => Err(cancelled()),
            out = work => out,
        }
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.turns.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap()
    }

    #[test]
    fn cancelling_interrupts_the_work_and_frees_the_slot() {
        rt().block_on(async {
            let turns = Arc::new(Turns::default());
            let guard = turns.begin("design:1").unwrap();
            assert!(turns.begin("design:1").is_err(), "同一个作用域不能同时跑两轮");
            assert!(turns.begin("board:1").is_ok(), "不同作用域互不相干");

            let stopper = {
                let turns = turns.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    assert!(turns.cancel("design:1"));
                }
            };
            let slow = async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok::<_, String>("finished")
            };
            let (out, ()) = tokio::join!(guard.run(slow), stopper);
            let err = out.unwrap_err();
            assert!(err.starts_with("#cancelled#"), "{err}");
            assert!(guard.flag.is_cancelled(), "引擎那边的开关也要置位");

            // 取消之后这一轮里再开始的活也不该跑(比如修复循环的下一次调用)
            assert!(guard.run(async { Ok::<_, String>(1) }).await.is_err());

            drop(guard);
            assert!(!turns.cancel("design:1"), "这一轮结束后就没东西可取消了");
            assert!(turns.begin("design:1").is_ok(), "位置腾出来了");
        });
    }

    #[test]
    fn work_that_finishes_first_is_not_affected() {
        rt().block_on(async {
            let turns = Arc::new(Turns::default());
            let guard = turns.begin("research").unwrap();
            assert_eq!(guard.run(async { Ok::<_, String>(42) }).await, Ok(42));
            assert_eq!(guard.run(async { Err::<u8, _>("boom".to_string()) }).await, Err("boom".into()));
        });
    }
}
