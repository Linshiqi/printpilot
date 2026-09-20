//! 接口密钥的存取(做法照搬 velo 的 credential_store):
//! **只进系统凭据管理器**(Windows Credential Manager,DPAPI 背书),不写配置文件、不进日志、不下发到前端。

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;

use pp_common::provider::ProviderId;

pub trait CredentialStore: Send + Sync {
    fn save(&self, name: &str, secret: &str) -> Result<(), String>;
    /// 没有这一条返回 `Ok(None)`,不算错误。
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    fn delete(&self, name: &str) -> Result<(), String>;
}

pub fn entry_name(id: ProviderId) -> String {
    format!("provider-key:{}", id.as_str())
}

pub struct KeyringStore;

impl KeyringStore {
    const SERVICE: &'static str = "PrintPilot";
}

impl CredentialStore for KeyringStore {
    fn save(&self, name: &str, secret: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(Self::SERVICE, name).map_err(|e| e.to_string())?;
        entry.set_password(secret).map_err(|e| e.to_string())
    }

    fn get(&self, name: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(Self::SERVICE, name).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(Self::SERVICE, name).map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 内存实现:单测用(不碰本机的凭据库)。
#[cfg(test)]
#[derive(Default)]
pub struct MemStore(Mutex<HashMap<String, String>>);

#[cfg(test)]
impl CredentialStore for MemStore {
    fn save(&self, name: &str, secret: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(name.to_string(), secret.to_string());
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_store_lifecycle_and_missing_is_not_an_error() {
        let s = MemStore::default();
        let name = entry_name(ProviderId::Deepseek);
        assert_eq!(name, "provider-key:deepseek");
        assert_eq!(s.get(&name), Ok(None));
        s.save(&name, "sk-test").unwrap();
        assert_eq!(s.get(&name), Ok(Some("sk-test".into())));
        s.delete(&name).unwrap();
        s.delete(&name).unwrap(); // 删两次也不报错
        assert_eq!(s.get(&name), Ok(None));
    }

    /// 真实的系统凭据管理器往返。会在本机凭据库里短暂写入一条测试项,所以默认不跑:
    /// `cargo test -p printpilot -- --ignored keyring`
    #[test]
    #[ignore]
    fn keyring_store_roundtrip() {
        let s = KeyringStore;
        let name = "provider-key:__selftest__";
        s.save(name, "secret-123").unwrap();
        assert_eq!(s.get(name), Ok(Some("secret-123".into())));
        s.delete(name).unwrap();
        assert_eq!(s.get(name), Ok(None));
    }
}
