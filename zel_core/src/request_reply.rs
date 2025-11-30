pub mod client;
pub mod server;
pub mod json_rpc;

pub use client::*;
pub use server::*;

#[cfg(test)]
mod ztest;

pub use auth::{AuthGuard, DynAuthGuard};
pub use error::ServiceError;
pub use transport::TxRx;

use std::{fmt::Debug, marker::PhantomData};

use iroh::{
    PublicKey,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use log::error;
use serde::de::DeserializeOwned;

use connection::handle_connection;

use crate::request_reply::service::Service;

pub struct Handler<Req, Reply, Svc, State> {
    _req: PhantomData<Req>,
    _reply: PhantomData<Reply>,
    auth_guard: Option<Box<dyn DynAuthGuard>>,
    service: Svc,
    state: State,
}

impl<Req, Reply, Svc, State> Debug for Handler<Req, Reply, Svc, State>
where
    State: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handler")
            .field("_req", &self._req)
            .field("_reply", &self._reply)
            .field("auth_guard", &self.auth_guard)
            .field("state", &self.state)
            .finish()
    }
}

impl<Req, Reply, Svc, State> ProtocolHandler for Handler<Req, Reply, Svc, State>
where
    Req: Debug + Sync + Send + 'static + serde::Serialize + DeserializeOwned,
    Reply: Debug + Sync + Send + 'static + serde::Serialize + DeserializeOwned,
    State: Clone + Debug + Sync + Send + 'static,
    Svc: Service<Req, State, Response = Reply>,
    Svc::Response: serde::Serialize + Sync + Send + 'static,
{
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_id = connection.remote_id();
        self.check_auth(remote_id).await?;

        let service = self.service.clone();
        let state = self.state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(connection, service, state).await {
                error!("{e}");
            }
        });
        Ok(())
    }
}

impl<Req, Reply, Svc, State> Handler<Req, Reply, Svc, State>
where
    Svc: Service<Req, State, Response = Reply>,
{
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

    pub fn builder(service: Svc, state: State) -> Builder<Req, Reply, State, Svc> {
        Builder {
            guard: None,
            service,
            state,
            _p: PhantomData::default(),
        }
    }
}

pub struct Builder<Request, Reply, State, Svc>
where
    Svc: Service<Request, State>,
{
    service: Svc,
    state: State,
    guard: Option<Box<dyn DynAuthGuard>>,
    _p: PhantomData<(Request, Reply)>,
}

impl<Request, Reply, State, Svc> Builder<Request, Reply, State, Svc>
where
    Svc: Service<Request, State, Response = Reply>,
{
    pub fn auth_guard<T>(mut self, guard: T) -> Self
    where
        T: AuthGuard,
    {
        self.guard = Some(guard.into());
        self
    }

    pub fn build(self) -> Handler<Request, Reply, Svc, State> {
        let Self {
            guard,
            service,
            state,
            _p,
        } = self;
        Handler {
            _req: PhantomData::default(),
            _reply: PhantomData::default(),
            auth_guard: guard,
            service,
            state,
        }
    }
}
