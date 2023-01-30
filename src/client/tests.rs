use std::net::SocketAddr;

use super::*;

use jsonrpsee::{
    core::server::rpc_module::Methods,
    server::{RandomStringIdProvider, RpcModule, ServerBuilder, ServerHandle},
};

async fn dummy_server(module: impl Into<Methods>) -> (SocketAddr, ServerHandle) {
    let server = ServerBuilder::default()
        .set_id_provider(RandomStringIdProvider::new(16))
        .build("0.0.0.0:0")
        .await
        .unwrap();

    let addr = server.local_addr().unwrap();
    let handle = server.start(module).unwrap();

    (addr, handle)
}

fn echo_module() -> RpcModule<()> {
    let mut module = RpcModule::new(());
    module.register_method("echo", |params, _| {
        Ok(params.parse::<JsonValue>().unwrap())
    }).unwrap();
    module
}

#[tokio::test]
async fn test_basic() {
    let (addr, handle) = dummy_server(echo_module()).await;

    let client = Client::new(&[format!("ws://{addr}")]).await.unwrap();

    let result = client.request("echo", Params::new(Some("[1]"))).await.unwrap();

    assert_eq!(result.to_string(), "[1]");

    handle.stop().unwrap();
}
