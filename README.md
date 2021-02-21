# jsonrpc-v2

[![Docs](https://docs.rs/jsonrpc-v2/badge.svg)](https://docs.rs/jsonrpc-v2/)
[![Crates.io](https://img.shields.io/crates/v/jsonrpc-v2.svg)](https://crates.io/crates/jsonrpc-v2)

A very small and very fast JSON-RPC 2.0 server-focused framework.

Provides integrations for both `hyper` and `actix-web` (both 1.x and 2.x).
Enable features `actix-web-v1-integration`, `actix-web-v2-integration`, or `hyper-integration` depending on need.

`actix-web-v2-integration` is enabled by default. Make sure to add `default-features = false` if using `hyper` or `actix-web` 1.x.

Also see the `easy-errors` feature flag (not enabled by default). Enabling this flag will implement [`ErrorLike`](https://docs.rs/jsonrpc-v2/&#42;/jsonrpc_v2/trait.ErrorLike.html)
for anything that implements `Display`, and the display value will be provided in the `message` field of the JSON-RPC 2.0 `Error` response.

Otherwise, custom errors should implement [`ErrorLike`](https://docs.rs/jsonrpc-v2/&#42;/jsonrpc_v2/trait.ErrorLike.html) to map errors to the JSON-RPC 2.0 `Error` response.

Individual method handlers are `async` functions that can take various kinds of args (things that can be extracted from the request, like
the `Params` or `Data`), and should return a `Result<Item, Error>` where the `Item` is serializable. See examples below.

## Usage

```rust
use jsonrpc_v2::{Data, Error, Params, Server};

#[derive(serde::Deserialize)]
struct TwoNums {
    a: usize,
    b: usize,
}

async fn add(Params(params): Params<TwoNums>) -> Result<usize, Error> {
    Ok(params.a + params.b)
}

async fn sub(Params(params): Params<(usize, usize)>) -> Result<usize, Error> {
    Ok(params.0 - params.1)
}

async fn message(data: Data<String>) -> Result<String, Error> {
    Ok(String::from(&*data))
}

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    let rpc = Server::new()
        .with_data(Data::new(String::from("Hello!")))
        .with_method("sub", sub)
        .with_method("message", message)
        .finish();

    actix_web::HttpServer::new(move || {
        let rpc = rpc.clone();
        actix_web::App::new().service(
            actix_web::web::service("/api")
                .guard(actix_web::guard::Post())
                .finish(rpc.into_web_service()),
        )
    })
    .bind("0.0.0.0:3000")?
    .run()
    .await
}
```

Current version: 0.9.1

License: MIT
