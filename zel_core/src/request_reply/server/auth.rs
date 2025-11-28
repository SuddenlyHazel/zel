use std::{future::Future, pin::Pin};

use iroh::PublicKey;

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
