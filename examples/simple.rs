use jsonrpc_v2::*;

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

async fn message(state: State<String>) -> Result<String, Error> {
    Ok(String::from(&*state))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = Server::with_state(String::from("Hello!"))
        .with_method("add", add)
        .with_method("sub", sub)
        .with_method("message", message)
        .finish_direct();

    let req = RequestObject::request()
        .with_method("sub")
        .with_params(serde_json::json!([12, 12]))
        .finish()?;

    let res = rpc.handle(req).await.unwrap();

    println!("{}", serde_json::to_string_pretty(&res)?);

    Ok(())
}
