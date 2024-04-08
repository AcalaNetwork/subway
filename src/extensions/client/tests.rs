use std::{str::FromStr, time::Duration};

use super::mock::*;
use super::*;

use futures::StreamExt;
use serde_json::json;
use tokio::sync::mpsc;

#[tokio::test]
async fn basic_request() {
    let (addr, handle, mut rx, _) = dummy_server().await;

    let client = Client::with_endpoints([format!("ws://{addr}")]).unwrap();

    let task = tokio::spawn(async move {
        let req = rx.recv().await.unwrap();
        assert_eq!(req.params.to_string(), "[1]");
        req.respond(JsonValue::from_str("[1]").unwrap());
    });

    let result = client.request("mock_rpc", vec![1.into()]).await.unwrap();

    assert_eq!(result.to_string(), "[1]");

    handle.stop().unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn basic_subscription() {
    let (addr, handle, _, mut rx) = dummy_server().await;

    let client = Client::with_endpoints([format!("ws://{addr}")]).unwrap();

    let task = tokio::spawn(async move {
        let sub = rx.recv().await.unwrap();
        assert_eq!(sub.params, json!([123]));
        sub.send(json!(10)).await;
        sub.send(json!(11)).await;
        sub.send(json!(12)).await;
    });

    let result = client
        .subscribe("mock_sub", vec![123.into()], "mock_unsub")
        .await
        .unwrap();

    let result = result.map(|v| v.unwrap().to_string()).take(3).collect::<Vec<_>>().await;

    assert_eq!(result, ["10", "11", "12"]);

    handle.stop().unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn multiple_endpoints() {
    // create 3 dummy servers
    let (addr1, handle1, rx1, _) = dummy_server().await;
    let (addr2, handle2, rx2, _) = dummy_server().await;
    let (addr3, handle3, rx3, _) = dummy_server().await;

    let client = Client::with_endpoints([
        format!("ws://{addr1}"),
        format!("ws://{addr2}"),
        format!("ws://{addr3}"),
    ])
    .unwrap();

    let handle_requests = |mut rx: mpsc::Receiver<MockRequest>, n: u32| {
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                req.respond(JsonValue::Number(n.into()));
            }
        })
    };

    let handler1 = handle_requests(rx1, 1);
    let handler2 = handle_requests(rx2, 2);
    let handler3 = handle_requests(rx3, 3);

    let result = client.request("mock_rpc", vec![11.into()]).await.unwrap();

    assert_eq!(result.to_string(), "1");

    handle1.stop().unwrap();

    let result = client.request("mock_rpc", vec![22.into()]).await.unwrap();

    assert_eq!(result.to_string(), "2");

    client.rotate_endpoint().await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = client.request("mock_rpc", vec![33.into()]).await.unwrap();

    assert_eq!(result.to_string(), "3");

    handle3.stop().unwrap();

    let result = client.request("mock_rpc", vec![44.into()]).await.unwrap();

    assert_eq!(result.to_string(), "2");

    handle2.stop().unwrap();

    let (r1, r2, r3) = tokio::join!(handler1, handler2, handler3);
    r1.unwrap();
    r2.unwrap();
    r3.unwrap();
}

#[tokio::test]
async fn concurrent_requests() {
    let (addr, handle, mut rx, _) = dummy_server().await;

    let client = Client::with_endpoints([format!("ws://{addr}")]).unwrap();

    let task = tokio::spawn(async move {
        let req1 = rx.recv().await.unwrap();
        let req2 = rx.recv().await.unwrap();
        let req3 = rx.recv().await.unwrap();

        req1.respond(JsonValue::from_str("1").unwrap());
        req2.respond(JsonValue::from_str("2").unwrap());
        req3.respond(JsonValue::from_str("3").unwrap());
    });

    let res1 = client.request("mock_rpc", vec![]);
    let res2 = client.request("mock_rpc", vec![]);
    let res3 = client.request("mock_rpc", vec![]);

    let res = tokio::join!(res1, res2, res3);

    assert_eq!(res.0.unwrap().to_string(), "1");
    assert_eq!(res.1.unwrap().to_string(), "2");
    assert_eq!(res.2.unwrap().to_string(), "3");

    handle.stop().unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn retry_requests_successful() {
    let (addr1, handle1, mut rx1, _) = dummy_server().await;
    let (addr2, handle2, mut rx2, _) = dummy_server().await;

    let client = Client::new(
        [format!("ws://{addr1}"), format!("ws://{addr2}")],
        Some(Duration::from_millis(100)),
        None,
        Some(2),
        None,
    )
    .unwrap();

    let h1 = tokio::spawn(async move {
        let _req = rx1.recv().await.unwrap();
        // no response, let it timeout
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let h2 = tokio::spawn(async move {
        let req = rx2.recv().await.unwrap();
        req.respond(json!(1));
    });

    let h3 = tokio::spawn(async move {
        let res = client.request("mock_rpc", vec![]).await.unwrap();
        assert_eq!(res.to_string(), "1");
    });

    h3.await.unwrap();
    h2.await.unwrap();
    h1.await.unwrap();

    handle1.stop().unwrap();
    handle2.stop().unwrap();
}

#[tokio::test]
async fn retry_requests_out_of_retries() {
    let (addr1, handle1, mut rx1, _) = dummy_server().await;
    let (addr2, handle2, mut rx2, _) = dummy_server().await;

    let client = Client::new(
        [format!("ws://{addr1}"), format!("ws://{addr2}")],
        Some(Duration::from_millis(100)),
        None,
        Some(2),
        None,
    )
    .unwrap();

    let h1 = tokio::spawn(async move {
        let _req = rx1.recv().await.unwrap();
        // no response, let it timeout
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let h2 = tokio::spawn(async move {
        let _req = rx2.recv().await.unwrap();
        // no response, let it timeout
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let h3 = tokio::spawn(async move {
        let res = client.request("mock_rpc", vec![]).await;
        assert_eq!(res.unwrap_err().data().unwrap().to_string(), "\"Request timeout\"");
    });

    h3.await.unwrap();
    h1.await.unwrap();
    h2.await.unwrap();

    handle1.stop().unwrap();
    handle2.stop().unwrap();
}

#[tokio::test]
async fn health_check_works() {
    let (addr1, handle1) = dummy_server_extend(Box::new(|builder| {
        let mut system_health = builder.register_method("system_health");
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(req) = system_health.recv() => {
                        req.respond(json!({ "isSyncing": true, "peers": 1, "shouldHavePeers": true }));
                    }
                }
            }
        });
    }))
    .await;

    let (addr2, handle2) = dummy_server_extend(Box::new(|builder| {
        let mut system_health = builder.register_method("system_health");
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(req) = system_health.recv() => {
                        req.respond(json!({ "isSyncing": false, "peers": 1, "shouldHavePeers": true }));
                    }
                }
            }
        });
    }))
    .await;

    let client = Client::new(
        [format!("ws://{addr1}"), format!("ws://{addr2}")],
        None,
        None,
        None,
        Some(HealthCheckConfig {
            interval_sec: 1,
            healthy_response_time_ms: 250,
            health_method: Some("system_health".into()),
            response: Some(HealthResponse::Contains(vec![(
                "isSyncing".to_string(),
                Box::new(HealthResponse::Eq(false.into())),
            )])),
        }),
    )
    .unwrap();

    // first endpoint is stale
    let res = client.request("system_health", vec![]).await;
    assert_eq!(
        res.unwrap(),
        json!({ "isSyncing": true, "peers": 1, "shouldHavePeers": true })
    );

    // wait for the health check to run
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1_050)).await;
    })
    .await
    .unwrap();

    // second endpoint is healthy
    let res = client.request("system_health", vec![]).await;
    assert_eq!(
        res.unwrap(),
        json!({ "isSyncing": false, "peers": 1, "shouldHavePeers": true })
    );

    handle1.stop().unwrap();
    handle2.stop().unwrap();
}
