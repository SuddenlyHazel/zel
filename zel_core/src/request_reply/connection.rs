use std::sync::Arc;

use anyhow::Context;
use futures::{SinkExt, StreamExt};
use iroh::{PublicKey, endpoint::Connection};
use log::{info, warn};
use serde::{Serialize, de::DeserializeOwned};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use super::{service::DynService, transport::TxRx};

pub(crate) async fn handle_connection<Req, Reply>(
    connection: Connection,
    service: Arc<Box<dyn DynService<Request = Req, Reply = Reply>>>,
) -> anyhow::Result<()>
where
    Req: DeserializeOwned + 'static,
    Reply: Serialize + 'static,
{
    let peer_id: PublicKey = connection.remote_id();
    let (send, recv) = connection
        .accept_bi()
        .await
        .context(format!("failed to open bidi channel for peer {peer_id}"))?;

    info!("accepted request/reply connection for peer {peer_id}");

    let codec = LengthDelimitedCodec::new();
    let mut framed = Framed::new(TxRx { send, recv }, codec);

    while let Some(Ok(bytes)) = framed.next().await {
        let Ok(request) = serde_json::from_slice::<Req>(&bytes) else {
            warn!("remote peer {peer_id} sent bad request");
            continue;
        };

        let service = service.as_ref();
        let resp = serde_json::to_vec(&service.serve(peer_id, request).await)
            .context("failed to serialize the response something bad is happening")?;

        if let Err(e) = framed.send(resp.into()).await {
            warn!(
                "failed to send response to peer {peer_id}. close_reason_maybe {:?}, error {e}",
                connection.close_reason()
            );
        }
    }
    Ok(())
}
