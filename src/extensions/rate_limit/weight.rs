use crate::config::RpcMethod;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct MethodWeights(Arc<BTreeMap<String, u32>>);

impl MethodWeights {
    pub fn get(&self, method: &str) -> u32 {
        self.0.get(method).cloned().unwrap_or(1)
    }
}

impl MethodWeights {
    pub fn from_config(methods: &[RpcMethod]) -> Self {
        let mut weights = BTreeMap::default();
        for method in methods {
            weights.insert(method.method.to_owned(), method.rate_limit_weight);
        }

        Self(Arc::new(weights))
    }
}
