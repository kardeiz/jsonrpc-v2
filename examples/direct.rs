use jsonrpc_v2::*;

#[derive(serde::Deserialize, serde::Serialize, Debug)]
struct TwoNums {
    a: usize,
    b: usize,
}

async fn add_ten(Params(TwoNums { a, b }): Params<TwoNums>) -> Result<TwoNums, Error> {
    Ok(TwoNums { a: a + 10, b: b + 10 })
}


async fn message(data: Data<String>) -> Result<String, Error> {
    Ok(String::from(&*data))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = Server::new()
        .with_data(Data::new(String::from("Hello!")))
        .with_method("add_ten", add_ten)
        .with_method("message", message)
        .finish();

    let req = RequestObject::request()
        .with_method("add_ten")
        .with_params(&[1, 2][..])
        .finish();

    match rpc.handle(req).await {
        ResponseObjects::One(ResponseObject::Result { result: s, ..}) => {
            let x: TwoNums = serde_json::from_value(serde_json::to_value(s)?)?;
            dbg!(x);
        },
        _ => {}
    };

    Ok(())
}
