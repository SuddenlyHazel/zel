use iroh::PublicKey;

#[derive(Debug, Clone)]
pub struct ConnectionExt {
    peer_id: PublicKey,
}

impl ConnectionExt {
    pub fn new(peer_id: PublicKey) -> Self {
        Self { peer_id }
    }
    pub fn peer(&self) -> &PublicKey {
        &self.peer_id
    }
}
