use crate::{Handler, IrohBundle, Service};

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

#[tokio::test]
async fn send_and_receive() -> anyhow::Result<()> {
    let server_bundle = IrohBundle::builder(None).await?.finish().await;
    let client_bundle = IrohBundle::builder(None).await?.finish().await;
    
    Ok(())
}
