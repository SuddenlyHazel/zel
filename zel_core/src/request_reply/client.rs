use std::marker::PhantomData;

use anyhow::anyhow;
use futures::{SinkExt, StreamExt};
use iroh::{
    Endpoint, PublicKey,
    endpoint::{ConnectError, ConnectionError},
};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::{ServiceError, request_reply::TxRx};

pub struct Client<Req, Reply> {
    _req: PhantomData<Req>,
    _reply: PhantomData<Reply>,
    framed: Framed<TxRx, LengthDelimitedCodec>,
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("failed to connect to endpoint {0}")]
    ConnectionError(#[from] ConnectionError),
    #[error("failed to connect to endpoint {0}")]
    ConnectError(#[from] ConnectError),
    #[error("failed to serialize payload {0}")]
    SerializeError(#[from] serde_json::Error),
    #[error("{0}")]
    ReadWrite(#[from] std::io::Error),
    #[error("{0}")]
    Reply(#[from] anyhow::Error),
    #[error("{0}")]
    InvalidResponse(anyhow::Error),
    #[error("{0}")]
    ServiceError(#[from] ServiceError),
}

impl<Req, Reply> Client<Req, Reply>
where
    Req: Serialize,
    Reply: DeserializeOwned,
{
    pub async fn request(&mut self, request: &Req) -> Result<Reply, ClientError> {
        let request = serde_json::to_vec(request)?;
        self.framed.send(request.into()).await?;

        let Some(Ok(bytes)) = self.framed.next().await else {
            return Err(ClientError::Reply(anyhow!(
                "failed to read data from the server"
            )));
        };

        let Ok(reply) = serde_json::from_slice::<Result<Reply, ServiceError>>(&bytes) else {
            return Err(ClientError::InvalidResponse(anyhow!(
                "server sent an invalid reply for request"
            )));
        };

        Ok(reply?)
    }
}

pub async fn new_client<Request, Response>(
    endpoint: Endpoint,
    peer: PublicKey,
    alpn: &[u8],
) -> Result<Client<Request, Response>, ClientError> {
    let conn = endpoint.connect(peer, alpn).await?;
    let (send, recv) = conn.open_bi().await?;

    let txrx = TxRx::new(send, recv);

    let codec = LengthDelimitedCodec::new();

    let framed = Framed::new(txrx, codec);

    Ok(Client {
        _req: PhantomData::default(),
        _reply: PhantomData::default(),
        framed,
    })
}
