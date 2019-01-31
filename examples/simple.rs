use jsonrpc_v2::*;
use futures::future::Future;

fn add(Params(params): Params<(usize, usize)>, state: State<AppState>) -> Result<usize, Error> {
    Ok(params.0 + params.1 + state.num)
}

fn forty_two(_params: Params<()>, _ctx: ()) -> Result<usize, Error> {
    Ok(42)
}

// fn subtract(Params(params): Params<(usize, usize)>, state: State<(usize,)>) -> Result<usize, Error> {
//     Ok(params.0 - params.1 - state.0)
// }

pub struct AppState {
    num: usize
}


fn main() -> Result<(), Box<dyn std::error::Error>> {
    
    let req: RawRequestObject = serde_json::from_str(r#"{"jsonrpc": "2.0", "method": "forty_two", "id": 10000}"#).unwrap();

    println!("{:?}", &req);

    let server = Server::new()
        .with_state(AppState { num: 23 })
        // .with_method("subtract".into(), subtract)
        .with_method("forty_two".into(), forty_two)
        .with_method("add".into(), add);

    let fut = server.handle(req)
        .map(|r| println!("{}", serde_json::to_string_pretty(&r).unwrap()) )
        .map_err(|e| println!("{:?}", e));

    tokio::run(fut);

    Ok(())
}