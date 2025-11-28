use std::{collections::BTreeMap, sync::Arc, time::Duration};

use iroh::PublicKey;
use tokio::sync::Mutex;

use crate::{
    Handler, IrohBundle, Service,
    request_reply::{new_client, service::fn_handler},
};

#[derive(Debug)]
pub struct EchoService {}

impl Service for EchoService {
    type Request = String;
    type Reply = String;

    async fn serve(
        &self,
        peer: iroh::PublicKey,
        request: Self::Request,
    ) -> Result<Self::Reply, super::ServiceError> {
        Ok(server_fmt(&request))
    }
}

fn server_fmt(v: &str) -> String {
    format!("server({v})")
}

static ALPN: &[u8] = b"zelcore/test/echo";

#[tokio::test]
async fn send_and_receive() -> anyhow::Result<()> {
    let _ = env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .try_init();
    let service = EchoService {};

    let handler = Handler::builder(service).build();

    let server_bundle = IrohBundle::builder(None).await?.accept(ALPN, handler);
    let server_bundle = server_bundle.finish().await;

    tokio::time::sleep(Duration::from_secs(1)).await;
    let client_bundle = IrohBundle::builder(None).await?.finish().await;

    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut client = new_client::<String, String>(
        client_bundle.endpoint.clone(),
        server_bundle.endpoint.id(),
        ALPN,
    )
    .await?;

    let m = "Heck".to_string();
    let r = client.request(&m).await?;
    assert_eq!(r, server_fmt("Heck"));

    let r1 = client.request(&r).await?;
    assert_eq!(r1, server_fmt(&r));

    Ok(())
}

#[tokio::test]
async fn fn_handler_send_and_receive() -> anyhow::Result<()> {
    let _ = env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .try_init();

    let data: BTreeMap<PublicKey, String> = BTreeMap::new();
    let data = Arc::new(Mutex::new(data));

    // Just demo + ensuring we can move state into server fns
    let service = fn_handler(move |peer, req: String| {
        let data = data.clone();
        async move {
            let mut locked = data.lock().await;
            locked.insert(peer, req.clone());
            Ok(server_fmt(&req))
        }
    });

    let handler = Handler::builder(service).build();

    let server_bundle = IrohBundle::builder(None).await?.accept(ALPN, handler);
    let server_bundle = server_bundle.finish().await;

    tokio::time::sleep(Duration::from_secs(1)).await;
    let client_bundle = IrohBundle::builder(None).await?.finish().await;

    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut client = new_client::<String, String>(
        client_bundle.endpoint.clone(),
        server_bundle.endpoint.id(),
        ALPN,
    )
    .await?;

    let m = "Heck".to_string();
    let r = client.request(&m).await?;
    assert_eq!(r, server_fmt("Heck"));

    let r1 = client.request(&r).await?;
    assert_eq!(r1, server_fmt("server(Heck)"));
    tokio::time::sleep(Duration::from_secs(1)).await;

    Ok(())
}

#[tokio::test]
async fn multiple_clients() -> anyhow::Result<()> {
    let _ = env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .try_init();

    let data: BTreeMap<PublicKey, String> = BTreeMap::new();
    let data = Arc::new(Mutex::new(data));

    // Service that tracks which peers have connected
    let _data = data.clone();
    let service = fn_handler(move |peer, req: String| {
        let data = _data.clone();
        async move {
            let mut locked = data.lock().await;
            locked.insert(peer, req.clone());
            Ok(server_fmt(&req))
        }
    });

    let handler = Handler::builder(service).build();

    let server_bundle = IrohBundle::builder(None).await?.accept(ALPN, handler);
    let server_bundle = server_bundle.finish().await;

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Create multiple clients
    let client_bundle_1 = IrohBundle::builder(None).await?.finish().await;
    let client_bundle_2 = IrohBundle::builder(None).await?.finish().await;
    let client_bundle_3 = IrohBundle::builder(None).await?.finish().await;

    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut client_1 = new_client::<String, String>(
        client_bundle_1.endpoint.clone(),
        server_bundle.endpoint.id(),
        ALPN,
    )
    .await?;

    let mut client_2 = new_client::<String, String>(
        client_bundle_2.endpoint.clone(),
        server_bundle.endpoint.id(),
        ALPN,
    )
    .await?;

    let mut client_3 = new_client::<String, String>(
        client_bundle_3.endpoint.clone(),
        server_bundle.endpoint.id(),
        ALPN,
    )
    .await?;

    // Send requests from all clients concurrently
    let msg1 = "Client1".to_string();
    let msg2 = "Client2".to_string();
    let msg3 = "Client3".to_string();

    let (r1, r2, r3) = tokio::join!(
        client_1.request(&msg1),
        client_2.request(&msg2),
        client_3.request(&msg3),
    );

    assert_eq!(r1?, server_fmt("Client1"));
    assert_eq!(r2?, server_fmt("Client2"));
    assert_eq!(r3?, server_fmt("Client3"));

    {
        let locked = data.lock().await;
        let one = locked.get(&client_bundle_1.endpoint.id());
        let two = locked.get(&client_bundle_2.endpoint.id());
        let three = locked.get(&client_bundle_3.endpoint.id());
        assert_eq!(one, Some(&"Client1".to_string()));
        assert_eq!(two, Some(&"Client2".to_string()));
        assert_eq!(three, Some(&"Client3".to_string()));
    }

    // Send another round of requests
    let msg1 = "Message1".to_string();
    let msg2 = "Message2".to_string();
    let msg3 = "Message3".to_string();

    let (r1, r2, r3) = tokio::join!(
        client_1.request(&msg1),
        client_2.request(&msg2),
        client_3.request(&msg3),
    );

    assert_eq!(r1?, server_fmt("Message1"));
    assert_eq!(r2?, server_fmt("Message2"));
    assert_eq!(r3?, server_fmt("Message3"));

    tokio::time::sleep(Duration::from_secs(1)).await;

    {
        let locked = data.lock().await;

        let one = locked.get(&client_bundle_1.endpoint.id());
        let two = locked.get(&client_bundle_2.endpoint.id());
        let three = locked.get(&client_bundle_3.endpoint.id());

        assert_eq!(one, Some(&"Message1".to_string()));
        assert_eq!(two, Some(&"Message2".to_string()));
        assert_eq!(three, Some(&"Message3".to_string()));
    }

    Ok(())
}
