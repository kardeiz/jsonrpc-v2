use jsonrpc_v2::*;
use futures::future::Future;
use actix::prelude::*;

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
    
    let sys = System::new("example");

    let req = RequestBytes(br#"[{"jsonrpc": "2.0", "method": "aadd", "params": [42, 23], "id": "1"}"#);

    // println!("{:?}", &req);

    let server = Server::new()
        .with_state(AppState { num: 23 })
        .with_method("forty_two".into(), forty_two)
        .with_method("add".into(), add);

    let addr = server.start();

    let result = addr.send(req);

    Arbiter::spawn(
        result.map(|res| {
            match res {
                Ok(result) => println!("Got result: {}", serde_json::to_string_pretty(&result).unwrap()),
                Err(err) => println!("Got error: {:?}", err),
            }
            System::current().stop();
        })
        .map_err(|e| {
            println!("Actor is probably died: {}", e);
        }));

    sys.run();

    // let fut = server.handle(req)
    //     .map(|r| println!("{}", serde_json::to_string_pretty(&r).unwrap()) )
    //     .map_err(|e| println!("{:?}", e));

    // fut.wait();

    // tokio::run(fut);

    Ok(())
}