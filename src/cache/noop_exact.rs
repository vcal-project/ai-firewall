use crate::cache::exact::ExactCache;
use async_trait::async_trait;

#[derive(Clone, Debug)]
pub struct NoopExactCache;

#[async_trait]
impl ExactCache for NoopExactCache {
    async fn get(&self, _key: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn set(&self, _key: &str, _value: String) -> anyhow::Result<()> {
        Ok(())
    }
}
