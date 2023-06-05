// pub mod methods;
// pub mod subscriptions;

use crate::middleware::{
    registry::Registry,
    traits::{Middleware, Provider},
};

pub enum Request {
    Call,
    Subscriptions,
}

pub fn registry() {
    let mut registry = Registry::<Box<dyn Provider<Box<dyn Middleware>>>>::new();
}
