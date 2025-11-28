use std::{future::Future, pin::Pin};

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
