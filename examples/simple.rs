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

async fn message(data: Data<String>) -> Result<String, Error> {
    Ok(String::from(&*data))
}

#[jsonrpc_v2_method(wrapped_fn = "message_x", externify = true)]
pub async fn message_test(a: i32, b: i32) -> Result<String, Error> {
    Ok(format!("{} + {}", a, b))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = Server::new()
        .with_data(Data::new(String::from("Hello!")))
        .with_method("add", add)
        .with_method("sub", sub)
        .with_method("message", message)
        .with_method("message-test", |params| async move { message_x(params).await })
        .finish_unwrapped();

    let req = RequestObject::request()
        .with_method("sub")
        .with_params(serde_json::json!([12, 12]))
        .finish();

    let res = rpc.handle(req).await;

    println!("{}", serde_json::to_string_pretty(&res)?);

    let req = RequestObject::request()
        .with_method("message-test")
        .with_params(serde_json::json!([12, 1]))
        .finish();

    let res = rpc.handle(req).await;

    println!("{}", serde_json::to_string_pretty(&res)?);

    Ok(())
}
