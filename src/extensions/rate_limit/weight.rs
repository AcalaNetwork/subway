use crate::config::RpcMethod;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct MethodWeights(pub(crate) BTreeMap<String, u32>);

impl MethodWeights {
    pub fn add(&mut self, method: &str, weight: u32) {
        self.0.insert(method.to_owned(), weight);
    }

    pub fn get(&self, method: &str) -> u32 {
        self.0.get(method).cloned().unwrap_or(1)
    }
}

impl MethodWeights {
    pub fn from_config(methods: &[RpcMethod]) -> Self {
        let mut weights = MethodWeights::default();
        for method in methods {
            weights.add(&method.method, method.rate_limit_weight);
        }
        weights
    }
}
