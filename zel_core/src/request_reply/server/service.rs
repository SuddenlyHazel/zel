use std::{fmt::Debug, marker::PhantomData, pin::Pin};

use iroh::PublicKey;

use crate::request_reply::ServiceError;

pub trait Service<Request, S>: Clone + Send + Sync + 'static {
    type Response;
    fn serve(
        &self,
        peer: PublicKey,
        request: Request,
        state: S,
    ) -> impl std::future::Future<Output = Result<Self::Response, ServiceError>> + std::marker::Send;
}

impl<F, Fut, Request, Response, State> Service<Request, State> for F
where
    F: Fn(PublicKey, Request, State) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Response, ServiceError>> + Send + 'static,
    Response: Clone + Send + Sync + 'static,
{
    //type Future = Fut;

    type Response = Response;

    fn serve(
        &self,
        peer: PublicKey,
        request: Request,
        state: State,
    ) -> impl std::future::Future<Output = Result<Response, ServiceError>> + std::marker::Send {
        (self.clone())(peer, request, state)
    }
}

pub struct ServiceFn<F, Request, Response, State> {
    inner: F,
    _p: PhantomData<(Request, Response, State)>,
}

impl<F, Request, Response, State> Debug for ServiceFn<F, Request, Response, State> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceFn").finish()
    }
}

impl<Request, Response, State, F, Fut> From<F> for ServiceFn<F, Request, Response, State>
where
    Fut: Future<Output = Result<Response, ServiceError>> + Send + Sync,
    F: Fn(PublicKey, Request, State) -> Fut + Send + Sync + 'static,
{
    fn from(inner: F) -> Self {
        Self {
            inner,
            _p: PhantomData::default(),
        }
    }
}

impl<F: Clone, Request, Response, State> Clone for ServiceFn<F, Request, Response, State> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _p: PhantomData,
        }
    }
}

impl<Request, Response, F, Fut, S> Service<Request, S> for ServiceFn<F, Request, Response, S>
where
    F: Fn(PublicKey, Request, S) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Response, ServiceError>> + Send + 'static,
    Response: Clone + Send + Sync + 'static,
    Request: Sync + Send + 'static,
    S: Sync + Send + Clone + 'static,
{
    type Response = Response;

    fn serve(
        &self,
        peer: PublicKey,
        request: Request,
        state: S,
    ) -> impl std::future::Future<Output = Result<Self::Response, ServiceError>> + std::marker::Send
    {
        (self.inner)(peer, request, state)
    }
}

// impl<F, Fut, Request, Response, State> Service<Request, State> for F
// where
//     F: Fn(PublicKey, Request, State) -> Fut + Clone + Send + Sync + 'static,
//     Fut: std::future::Future<Output = Result<Response, ServiceError>> + Send + 'static,
//     Response: Clone + Send + Sync + 'static,
//     Request: Sync + Send + 'static,
//     State: Sync + Send + Clone + 'static,
// {
//     type Response = Response;
// }

// pub trait DynService: Send + Sync + std::fmt::Debug + 'static {
//     type Request;
//     type Reply;
//     type State;
//     fn serve(
//         &self,
//         peer: PublicKey,
//         request: Self::Request,
//         state: Self::State,
//     ) -> Pin<Box<dyn Future<Output = Result<Self::Reply, ServiceError>> + std::marker::Send + '_>>;
// }
