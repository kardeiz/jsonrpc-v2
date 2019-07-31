# jsonrpc-v2

A very small and very fast JSON-RPC 2.0 server-focused framework.

Provides an integration for `actix-web` servers.

## Usage

```rust
use jsonrpc_v2::*;

#[derive(serde::Deserialize)]
struct TwoNums { a: usize, b: usize }

fn add(Params(params): Params<TwoNums>) -> Result<usize, Error> {
    Ok(params.a + params.b)
}

fn sub(Params(params): Params<(usize, usize)>) -> Result<usize, Error> {
    Ok(params.0 - params.1)
}

fn message(state: State<String>) -> Result<String, Error> {
    Ok(String::from(&*state))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {

    let rpc = Server::with_state(String::from("Hello!"))
        .with_method("add", add)
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
    .run()?;

    Ok(())
}
```

License: MIT
