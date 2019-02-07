use actix::prelude::*;
use futures::future::Future;
use jsonrpc_v2::*;

// use actix_web
use futures::future::IntoFuture;
// use actix_web::handler::AsyncResponder;

fn api(
    (bytes, rpc): (bytes::Bytes, actix_web::State<actix::Addr<Server<AppState>>>)
) -> impl Future<Item = actix_web::HttpResponse, Error = actix_web::Error> {
    rpc.send(RequestBytes(bytes)).from_err().and_then(|res| {
        match res {
            Ok(json) => Ok(actix_web::HttpResponse::Ok().json(json)),
            Err(_) => Ok(actix_web::HttpResponse::InternalServerError().into())
        }
    })
}

fn add(Params(params): Params<(usize, usize)>, state: State<AppState>) -> Result<usize, Error> {
    Ok(params.0 + params.1 + state.num)
}

fn forty_two(_params: Params<()>, ctx: ()) -> Result<usize, Error> { Err(42.into()) }

// fn subtract(Params(params): Params<(usize, usize)>, state: State<(usize,)>) -> Result<usize, Error> {
//     Ok(params.0 - params.1 - state.0)
// }

pub struct AppState {
    num: usize
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // let sys = System::new("example");

    // let req = RequestBytes(br#"[{"jsonrpc": "2.0", "method": "add", "params": [42, 23], "id": "1"}"#);

    // println!("{:?}", &req);

    let jsonrpc = Server::with_state(AppState { num: 23 })
        .with_method("forty_two", forty_two)
        .with_method("add", add)
        .finish();

    // jsonrpc.to();

    // let addr = actix::Arbiter::start(|_| server);

    // let addr = jsonrpc.start();

    actix_web::server::new(move || {
        let jsonrpc = jsonrpc.clone();
        actix_web::App::with_state(())
            .resource("/api", |r| r.method(http::Method::POST).h(jsonrpc))
    })
    .bind("0.0.0.0:3000")
    .unwrap()
    .run();

       // let server = Server::with_state(AppState { num: 23 })
       //  .with_method("forty_two", forty_two)
       //  .with_method("add", add);

    // let addr = actix::Arbiter::start(|_| server);



    /*actix_web::server::new(move || {
        actix_web::App::with_state(addr.clone())
            .resource("/api", |r| r.method(http::Method::POST).with_async(api))
    })
    .bind("0.0.0.0:3000")
    .unwrap()
    .run();*/

    // let addr = server.start();

    // let result = addr.send(req);

    // Arbiter::spawn(
    //     result.map(|res| {
    //         match res {
    //             Ok(result) => println!("Got result: {}", serde_json::to_string_pretty(&result).unwrap()),
    //             Err(err) => println!("Got error: {:?}", err),
    //         }
    //         System::current().stop();
    //     })
    //     .map_err(|e| {
    //         println!("Actor is probably died: {}", e);
    //     }));

    // sys.run();

    // let fut = server.handle(req)
    //     .map(|r| println!("{}", serde_json::to_string_pretty(&r).unwrap()) )
    //     .map_err(|e| println!("{:?}", e));

    // fut.wait();

    // tokio::run(fut);

    Ok(())
}
