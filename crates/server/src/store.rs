use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;

use crate::model::AppStateData;

#[derive(Clone)]
pub struct Store {
    path: PathBuf,
    inner: Arc<RwLock<AppStateData>>,
}

impl Store {
    pub async fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        tokio::fs::create_dir_all(data_dir)
            .await
            .with_context(|| format!("创建数据目录失败: {}", data_dir.display()))?;
        let path = data_dir.join("config.json");
        let mut data = if path.exists() {
            let raw = tokio::fs::read_to_string(&path)
                .await
                .with_context(|| format!("读取配置失败: {}", path.display()))?;
            serde_json::from_str(&raw).with_context(|| "解析 config.json 失败")?
        } else {
            let data = AppStateData::default_skeleton();
            let pretty = serde_json::to_string_pretty(&data)?;
            tokio::fs::write(&path, pretty)
                .await
                .with_context(|| format!("写入默认配置失败: {}", path.display()))?;
            data
        };
        data.profile.normalize();
        Ok(Self {
            path,
            inner: Arc::new(RwLock::new(data)),
        })
    }

    pub async fn get(&self) -> AppStateData {
        self.inner.read().await.clone()
    }

    pub async fn replace(&self, mut data: AppStateData) -> Result<()> {
        data.profile.normalize();
        let pretty = serde_json::to_string_pretty(&data)?;
        tokio::fs::write(&self.path, &pretty)
            .await
            .with_context(|| format!("保存配置失败: {}", self.path.display()))?;
        *self.inner.write().await = data;
        Ok(())
    }

    pub async fn update<F>(&self, f: F) -> Result<AppStateData>
    where
        F: FnOnce(&mut AppStateData),
    {
        let mut guard = self.inner.write().await;
        f(&mut guard);
        guard.profile.normalize();
        let pretty = serde_json::to_string_pretty(&*guard)?;
        tokio::fs::write(&self.path, &pretty)
            .await
            .with_context(|| format!("保存配置失败: {}", self.path.display()))?;
        Ok(guard.clone())
    }
}
