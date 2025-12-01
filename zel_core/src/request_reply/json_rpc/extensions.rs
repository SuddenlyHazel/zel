use iroh::{
    PublicKey,
    endpoint::{Connection, ConnectionStats},
};

#[derive(Debug, Clone)]
pub struct ConnectionExt {
    connection: Connection,
}

impl ConnectionExt {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }
    pub fn peer(&self) -> PublicKey {
        self.connection.remote_id()
    }
    pub fn stats(&self) -> ConnectionStats {
        self.connection.stats()
    }
}
