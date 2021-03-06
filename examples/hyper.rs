use jsonrpc_v2::*;

#[derive(serde::Deserialize)]
struct TwoNums {
    a: usize,
    b: usize,
}

async fn add(Params(params): Params<TwoNums>) -> Result<usize, Error> {
    Ok(params.a + params.b)
}

async fn sub(
    Params(params): Params<(usize, usize)>,
    id: Option<jsonrpc_v2::Id>,
    method: jsonrpc_v2::Method,
) -> Result<usize, Error> {
    dbg!(id);
    dbg!(method.as_str());
    Ok(params.0 - params.1)
}

async fn message(data: Data<String>) -> Result<String, Error> {
    Ok(String::from(&*data))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = Server::new()
        .with_data(Data::new(String::from("Hello!")))
        .with_method("add", add)
        .with_method("sub", sub)
        .with_method("message", message)
        .finish();

    let addr = "0.0.0.0:3000".parse().unwrap();

    let server = hyper::server::Server::bind(&addr).serve(rpc.into_hyper_web_service());

    server.await?;

    Ok(())
}
