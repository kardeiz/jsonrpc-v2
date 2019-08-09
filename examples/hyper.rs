use futures::future::Future;
use jsonrpc_v2::*;

#[derive(serde::Deserialize)]
struct TwoNums {
    a: usize,
    b: usize,
}

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

    let addr = "0.0.0.0:3000".parse().unwrap();

    let server = hyper::Server::bind(&addr)
        .serve(rpc.into_hyper_web_service())
        .map_err(|e| eprintln!("server error: {}", e));

    hyper::rt::run(server);

    Ok(())
}
