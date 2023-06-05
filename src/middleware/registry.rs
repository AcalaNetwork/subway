use std::collections::HashMap;

pub struct Registry<T> {
    registry: HashMap<String, T>,
}

impl<T> Registry<T> {
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
        }
    }

    pub fn register(&mut self, name: String, middleware: T) {
        assert!(
            self.registry.insert(name, middleware).is_none(),
            "Already registered"
        );
    }

    pub fn get(&self, name: &String) -> Option<&T> {
        self.registry.get(name)
    }
}
