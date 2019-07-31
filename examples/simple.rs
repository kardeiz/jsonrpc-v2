use jsonrpc_v2::*;
use futures::future::Future;

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

    let req = RequestObject::notification()
        .with_method("sub")
        .with_params(serde_json::json!([12, 12]))
        .finish()?;

    let res = rpc.handle(req).wait().unwrap();

    println!("{}", serde_json::to_string_pretty(&res)?);

    Ok(())
}
