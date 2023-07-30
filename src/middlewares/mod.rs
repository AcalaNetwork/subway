// pub mod methods;
// pub mod subscriptions;

use crate::middleware::{
    registry::Registry,
    traits::{Middleware, MiddlewareBuilder},
};

pub enum Request {
    Call,
    Subscriptions,
}

type Response = String;

pub fn registry() {
    // let mut registry =
    // Registry::<Box<dyn MiddlewareBuilder<Box<dyn Middleware<Request, Response>>>>>::new();
}
