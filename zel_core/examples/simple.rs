use derive_more::TryInto;
use iroh::PublicKey;
use serde::{Deserialize, Serialize};

mod request {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    struct ReqHello(String);

    #[derive(Debug, Serialize, Deserialize)]
    struct ReqGoodbye(String);

    #[derive(Debug, Serialize, Deserialize, TryInto)]
    enum Request {
        Hello(ReqHello),
        Goodbye(ReqGoodbye),
    }
}

mod response {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    struct RespHello(String);

    #[derive(Debug, Serialize, Deserialize)]
    struct RespGoodbye(String);

    #[derive(Debug, Serialize, Deserialize, TryInto)]
    enum Response {
        Hello(RespHello),
        Goodbye(RespGoodbye),
    }
}

#[tokio::main]
pub async fn main() {}
