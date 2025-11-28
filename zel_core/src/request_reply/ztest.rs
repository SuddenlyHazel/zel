use std::time::Duration;

use crate::{Handler, IrohBundle, Service, request_reply::new_client};

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
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .init();
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
