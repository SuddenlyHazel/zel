use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum ServiceError {
    #[error("Not authorized.")]
    NotAuthorized {},
}
