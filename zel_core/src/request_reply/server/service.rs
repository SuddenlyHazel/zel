use std::{fmt::Debug, future::Future, marker::PhantomData, pin::Pin};

use iroh::PublicKey;

use super::error::ServiceError;

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

pub struct FnHandler<Req, Reply, F> {
    inner: F,
    _phantom: std::marker::PhantomData<(Req, Reply)>,
}

pub fn fn_handler<Req, Reply, F, Fut>(inner: F) -> FnHandler<Req, Reply, F>
where
    Fut: Future<Output = Result<Reply, ServiceError>> + Send + Sync,
    F: Fn(PublicKey, Req) -> Fut + Send + Sync + 'static,
{
    FnHandler {
        inner,
        _phantom: PhantomData::default(),
    }
}

impl<T, Req, Reply> Debug for FnHandler<T, Req, Reply> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FnHandler").finish()
    }
}

// impl<Req, Reply, P> Service for FnHandler<P>
// where
//     P: FnMut(Req) -> Reply,
// {
//     type Request = Req;
//     type Reply = Reply;

//     async fn serve(&self, peer: PublicKey, request: Req) -> Result<Reply, ServiceError> {
//         todo!()
//     }
// }

// impl<Req, Reply, Fn: FnMut(Req) -> Reply> From<FnHandler<Fn>>
//     for Box<dyn DynService<Request = Req, Reply = Reply>>
// {
//     fn from(value: FnHandler<Fn>) -> Self {
//         todo!()
//     }
// }

impl<Req, Reply, Fut, HandleFn> Service for FnHandler<Req, Reply, HandleFn>
where
    Req: Send + Sync + std::fmt::Debug + 'static,
    Reply: Send + Sync + std::fmt::Debug + 'static,
    Fut: Future<Output = Result<Reply, ServiceError>> + Send + Sync,
    HandleFn: Fn(PublicKey, Req) -> Fut + Send + Sync + 'static,
{
    type Request = Req;
    type Reply = Reply;

    async fn serve(
        &self,
        peer: PublicKey,
        request: Self::Request,
    ) -> Result<Self::Reply, ServiceError> {
        (self.inner)(peer, request).await
    }
}
