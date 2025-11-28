pub mod client;
pub mod server;

pub use client::*;
pub use server::*;

#[cfg(test)]
mod ztest;

pub use auth::{AuthGuard, DynAuthGuard};
pub use error::ServiceError;
pub use service::{DynService, Service};
pub use transport::TxRx;

use std::{fmt::Debug, marker::PhantomData, sync::Arc};

use iroh::{
    PublicKey,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use log::error;
use serde::de::DeserializeOwned;

use connection::handle_connection;

#[derive(Debug)]
pub struct Handler<Req, Reply> {
    _req: PhantomData<Req>,
    _reply: PhantomData<Reply>,
    auth_guard: Option<Box<dyn DynAuthGuard>>,
    service: Arc<Box<dyn DynService<Request = Req, Reply = Reply>>>,
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
