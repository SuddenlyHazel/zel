use std::{fmt::Debug, marker::PhantomData, pin::Pin, sync::Arc};

use anyhow::Context;
use futures::{SinkExt, StreamExt};
use iroh::{
    PublicKey,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use log::{error, info, warn};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

#[derive(Debug)]
pub struct Handler<Req, Reply> {
    _req: PhantomData<Req>,
    _reply: PhantomData<Reply>,
    auth_guard: Option<Box<dyn DynAuthGuard>>,
    service: Arc<Box<dyn DynService<Request = Req, Reply = Reply>>>,
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum ServiceError {
    #[error("Not authorized.")]
    NotAuthorized {},
}

pub trait AuthGuard: Send + Sync + std::fmt::Debug + 'static {
    fn check(
        &self,
        remote_id: PublicKey,
    ) -> impl std::future::Future<Output = bool> + std::marker::Send;
}

pub trait DynAuthGuard: Send + Sync + std::fmt::Debug + 'static {
    fn check(
        &self,
        remote_id: PublicKey,
    ) -> Pin<Box<dyn Future<Output = bool> + std::marker::Send + '_>>;
}

impl<P: AuthGuard> DynAuthGuard for P {
    fn check(
        &self,
        remote_id: PublicKey,
    ) -> Pin<Box<dyn Future<Output = bool> + std::marker::Send + '_>> {
        Box::pin(<Self as AuthGuard>::check(self, remote_id))
    }
}

impl<T: AuthGuard> From<T> for Box<dyn DynAuthGuard> {
    fn from(value: T) -> Self {
        Box::new(value)
    }
}

pub trait Service: Send + Sync + std::fmt::Debug + 'static {
    type Request;
    type Reply;

    fn serve(
        &self,
        peer: PublicKey,
        request: Self::Request,
    ) -> impl std::future::Future<Output = Result<Self::Reply, ServiceError>> + std::marker::Send;
}

pub trait DynService: Send + Sync + std::fmt::Debug + 'static {
    type Request;
    type Reply;
    fn serve(
        &self,
        peer: PublicKey,
        request: Self::Request,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Reply, ServiceError>> + std::marker::Send + '_>>;
}

impl<P: Service> DynService for P {
    type Request = P::Request;

    type Reply = P::Reply;

    fn serve(
        &self,
        peer: PublicKey,
        request: Self::Request,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Reply, ServiceError>> + std::marker::Send + '_>>
    {
        let f = self.serve(peer, request);
        Box::pin(async move { f.await })
    }
}

impl<T: Service> From<T> for Box<dyn DynService<Request = T::Request, Reply = T::Reply>> {
    fn from(value: T) -> Self {
        Box::new(value)
    }
}

impl<Req, Reply> ProtocolHandler for Handler<Req, Reply>
where
    Req: Debug + Sync + Send + 'static + serde::Serialize + DeserializeOwned,
    Reply: Debug + Sync + Send + 'static + serde::Serialize + DeserializeOwned,
{
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_id = connection.remote_id();
        self.check_auth(remote_id).await?;

        let service = self.service.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(connection, service).await {
                error!("{e}");
            }
        });
        Ok(())
    }
}

async fn handle_connection<Req, Reply>(
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

pub struct TxRx {
    send: SendStream,
    recv: RecvStream,
}

impl AsyncWrite for TxRx {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let mut_self = self.get_mut();
        tokio::io::AsyncWrite::poll_write(Pin::new(&mut mut_self.send), cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let mut_self = self.get_mut();
        tokio::io::AsyncWrite::poll_flush(Pin::new(&mut mut_self.send), cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let mut_self = self.get_mut();
        tokio::io::AsyncWrite::poll_shutdown(Pin::new(&mut mut_self.send), cx)
    }
}

impl AsyncRead for TxRx {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let mut_self = self.get_mut();
        tokio::io::AsyncRead::poll_read(Pin::new(&mut mut_self.recv), cx, buf)
    }
}

impl<Req, Reply> Handler<Req, Reply> {
    async fn check_auth(&self, remote_id: PublicKey) -> Result<(), AcceptError> {
        // Get the guard or return ok if it isn't present
        let guard = match &self.auth_guard {
            Some(guard) => guard,
            None => return Ok(()),
        };

        let is_authorized = guard.check(remote_id).await;

        if is_authorized {
            return Ok(());
        }

        return Err(AcceptError::from_err(ServiceError::NotAuthorized {}));
    }

    pub fn builder<T>(service: T) -> Builder<Req, Reply>
    where
        T: Service<Request = Req, Reply = Reply>,
    {
        let service = service.into();
        Builder {
            guard: None,
            service,
        }
    }
}

pub struct Builder<Request, Reply> {
    service: Box<dyn DynService<Request = Request, Reply = Reply>>,
    guard: Option<Box<dyn DynAuthGuard>>,
}

impl<Request, Reply> Builder<Request, Reply> {
    pub fn auth_guard<T>(mut self, guard: T) -> Self
    where
        T: AuthGuard,
    {
        self.guard = Some(guard.into());
        self
    }

    pub fn build(self) -> Handler<Request, Reply> {
        let Self { guard, service } = self;
        Handler {
            _req: PhantomData::default(),
            _reply: PhantomData::default(),
            auth_guard: guard,
            service: Arc::new(service),
        }
    }
}
